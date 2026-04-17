use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::widgets::log_entry::{LogEntry, ToolResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollPos {
    /// Follow the tail — always show the newest content.
    #[default]
    Tail,
    /// Manual scroll, value is a source-line index used as the render start.
    Offset(usize),
}

#[allow(dead_code)] // Task 15 will remove this bridge; keeping new/push for now
pub struct LogBuffer {
    lines: Vec<String>,
    max_lines: usize,
}

#[allow(dead_code)] // Task 15 removes the bridge; keeping new/push until then
impl LogBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: Vec::new(),
            max_lines,
        }
    }

    pub fn push(&mut self, line: String) {
        self.lines.push(line);
        if self.lines.len() > self.max_lines {
            self.lines.remove(0);
        }
    }

    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub(crate) fn from_lines(lines: Vec<String>, max_lines: usize) -> Self {
        Self { lines, max_lines }
    }
}

// ---------------------------------------------------------------------------
// EntryBuffer — structured log storage (Task 9+)
// ---------------------------------------------------------------------------

pub struct EntryBuffer {
    entries: Vec<LogEntry>,
    max_entries: usize,
}

impl EntryBuffer {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    pub fn push(&mut self, entry: LogEntry) {
        self.entries.push(entry);
        if self.entries.len() > self.max_entries {
            self.entries.remove(0);
        }
    }

    /// Attach a tool_result to the most recent matching ToolUse entry.
    /// Returns true if a match was found.
    pub fn attach_result(&mut self, tool_use_id: &str, result: ToolResult) -> bool {
        for entry in self.entries.iter_mut().rev() {
            if let LogEntry::Tool {
                tool_use_id: id,
                result: slot,
                ..
            } = entry
                && id == tool_use_id
                && slot.is_none()
            {
                *slot = Some(result);
                return true;
            }
        }
        false
    }

    pub fn entries(&self) -> &[LogEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Number of visible rows a line will occupy when rendered with `Wrap { trim: false }`.
/// `width` is the interior width (area width minus left/right borders).
pub(crate) fn rendered_rows(line: &str, width: u16) -> usize {
    if width == 0 {
        return 1;
    }
    let w = width as usize;
    // Ratatui wraps by display width. For ASCII-heavy agent output this is close
    // enough to char count; if multi-byte content becomes common, swap to
    // unicode-width. Each explicit '\n' is its own row.
    line.split('\n')
        .map(|seg| {
            let chars = seg.chars().count().max(1);
            chars.div_ceil(w)
        })
        .sum()
}

fn start_for_tail(lines: &[String], visible_height: usize, width: u16) -> usize {
    // Walk backwards from the end, accumulating rendered rows, until we fill
    // the viewport. Return the source-line index to start rendering from.
    let mut rows = 0;
    let mut i = lines.len();
    while i > 0 && rows < visible_height {
        i -= 1;
        rows += rendered_rows(&lines[i], width);
    }
    // If we overshot visible_height, `i` is still the right start — the top
    // line may be partially clipped by ratatui's renderer, which is fine.
    i
}

fn line_style(line: &str) -> Style {
    if line.starts_with('[') {
        Style::default().fg(Color::Cyan)
    } else if line.contains("error") || line.contains("Error") {
        Style::default().fg(Color::Red)
    } else if line.contains("warning") || line.contains("Warning") {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default()
    }
}

pub fn render_log(
    frame: &mut Frame,
    area: Rect,
    buffer: &LogBuffer,
    scroll: ScrollPos,
    title: &str,
) {
    if buffer.is_empty() {
        let empty = Paragraph::new("No output yet.")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(title.to_string()),
            );
        frame.render_widget(empty, area);
        return;
    }

    let visible_height = area.height.saturating_sub(2) as usize; // minus borders
    let interior_width = area.width.saturating_sub(2); // minus borders
    let total = buffer.len();
    let all = buffer.lines();

    // Pick the start source-line index.
    let start = match scroll {
        ScrollPos::Tail => start_for_tail(all, visible_height, interior_width),
        ScrollPos::Offset(n) => n.min(total.saturating_sub(1)),
    };

    // Consume rendered-row budget forward from `start` to find how many source
    // lines we include.
    let mut rows_used = 0;
    let mut end = start;
    while end < total && rows_used < visible_height {
        rows_used += rendered_rows(&all[end], interior_width);
        end += 1;
    }

    let lines: Vec<Line> = all[start..end]
        .iter()
        .map(|line| {
            let style = line_style(line);
            Line::from(Span::styled(line.as_str(), style))
        })
        .collect();

    let scroll_indicator = if total <= visible_height && matches!(scroll, ScrollPos::Tail) {
        String::new()
    } else {
        let (line_no, pct) = match scroll {
            ScrollPos::Tail => (total, 100),
            ScrollPos::Offset(n) => {
                let line_no = (n + 1).min(total);
                let pct = if total <= 1 { 100 } else { (line_no * 100) / total };
                (line_no, pct)
            }
        };
        format!(" [{line_no}/{total} · {pct}%]")
    };

    let log = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{title}{scroll_indicator}")),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(log, area);
}

// ---------------------------------------------------------------------------
// Bridge helpers: EntryBuffer → flatten → LogBuffer → render_log (Task 15 removes)
// ---------------------------------------------------------------------------

/// Flatten EntryBuffer to lines for the (still flat) renderer. Phase 3 replaces this.
pub fn flatten_entries(buf: &EntryBuffer) -> Vec<String> {
    let mut out = Vec::new();
    for entry in buf.entries() {
        match entry {
            LogEntry::Text(s) => {
                for line in s.lines() {
                    out.push(line.to_string());
                }
            }
            LogEntry::Marker(s) => out.push(format!("[{s}]")),
            LogEntry::Tool {
                tool,
                input,
                result,
                ..
            } => {
                let preview = if input.chars().count() > 120 {
                    let safe_end = input
                        .char_indices()
                        .nth(120)
                        .map(|(i, _)| i)
                        .unwrap_or(input.len());
                    format!("{}...", &input[..safe_end])
                } else {
                    input.clone()
                };
                out.push(format!("→ {tool}: {preview}"));
                if let Some(r) = result {
                    let status = if r.is_error { "✗" } else { "✓" };
                    out.push(format!("  {status} ({}ms)", r.duration_ms));
                    for line in r.output.lines() {
                        out.push(format!("    {line}"));
                    }
                }
            }
        }
    }
    out
}

pub fn render_entries(
    frame: &mut Frame,
    area: Rect,
    buffer: &EntryBuffer,
    scroll: ScrollPos,
    title: &str,
) {
    if buffer.is_empty() {
        let empty = Paragraph::new("No output yet.")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title(title.to_string()));
        frame.render_widget(empty, area);
        return;
    }
    let lines = flatten_entries(buffer);
    let temp = LogBuffer::from_lines(lines, 5000);
    render_log(frame, area, &temp, scroll, title);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_log_buffer_push() {
        let mut buf = LogBuffer::new(3);
        buf.push("line 1".to_string());
        buf.push("line 2".to_string());
        assert_eq!(buf.len(), 2);
        assert!(!buf.is_empty());
    }

    #[test]
    fn test_log_buffer_max_lines() {
        let mut buf = LogBuffer::new(2);
        buf.push("a".to_string());
        buf.push("b".to_string());
        buf.push("c".to_string());
        assert_eq!(buf.len(), 2);
        assert_eq!(buf.lines()[0], "b");
        assert_eq!(buf.lines()[1], "c");
    }

    #[test]
    fn test_log_buffer_new_empty() {
        let buf = LogBuffer::new(100);
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
    }

    #[test]
    fn test_rendered_rows_short_line() {
        assert_eq!(rendered_rows("hello", 80), 1);
    }

    #[test]
    fn test_rendered_rows_wraps() {
        assert_eq!(rendered_rows(&"x".repeat(161), 80), 3);
    }

    #[test]
    fn test_rendered_rows_empty() {
        assert_eq!(rendered_rows("", 80), 1);
    }

    #[test]
    fn test_rendered_rows_zero_width_defensive() {
        assert_eq!(rendered_rows("hello", 0), 1);
    }

    #[test]
    fn test_rendered_rows_embedded_newline() {
        assert_eq!(rendered_rows("a\nb", 80), 2);
    }

    #[test]
    fn test_start_for_tail_fits_without_wrap() {
        let lines: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        assert_eq!(start_for_tail(&lines, 5, 80), 5);
    }

    #[test]
    fn test_start_for_tail_with_wrapping_lines() {
        // 3 lines of width 160, viewport 80 wide → each takes 2 rendered rows.
        // Viewport height 4 fits 2 lines, so start should be lines.len() - 2.
        let lines: Vec<String> = (0..3).map(|_| "x".repeat(160)).collect();
        assert_eq!(start_for_tail(&lines, 4, 80), 1);
    }

    #[test]
    fn test_entry_buffer_attach_result() {
        use crate::tui::widgets::log_entry::{LogEntry, ToolResult};
        let mut buf = EntryBuffer::new(100);
        buf.push(LogEntry::Tool {
            tool_use_id: "toolu_01".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: None,
            started_at: std::time::Instant::now(),
        });
        let attached = buf.attach_result(
            "toolu_01",
            ToolResult {
                output: "ok".into(),
                is_error: false,
                duration_ms: 42,
            },
        );
        assert!(attached);
        match &buf.entries()[0] {
            LogEntry::Tool { result: Some(r), .. } => {
                assert_eq!(r.duration_ms, 42);
                assert!(!r.is_error);
            }
            _ => panic!("expected attached result"),
        }
    }

    #[test]
    fn test_entry_buffer_attach_ignores_already_filled() {
        use crate::tui::widgets::log_entry::{LogEntry, ToolResult};
        let mut buf = EntryBuffer::new(100);
        buf.push(LogEntry::Tool {
            tool_use_id: "toolu_01".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: Some(ToolResult {
                output: "first".into(),
                is_error: false,
                duration_ms: 1,
            }),
            started_at: std::time::Instant::now(),
        });
        let attached = buf.attach_result(
            "toolu_01",
            ToolResult {
                output: "second".into(),
                is_error: true,
                duration_ms: 2,
            },
        );
        assert!(!attached);
    }
}
