use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollPos {
    /// Follow the tail — always show the newest content.
    #[default]
    Tail,
    /// Manual scroll, value is a source-line index used as the render start.
    Offset(usize),
}

pub struct LogBuffer {
    lines: Vec<String>,
    max_lines: usize,
}

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

}

/// Number of visible rows a line will occupy when rendered with `Wrap { trim: false }`.
/// `width` is the interior width (area width minus left/right borders).
#[allow(dead_code)] // integrated in Task 3
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

    let visible_height = area.height.saturating_sub(2) as usize; // subtract borders
    let total = buffer.len();

    let start = match scroll {
        ScrollPos::Tail => total.saturating_sub(visible_height),
        ScrollPos::Offset(n) => n.min(total.saturating_sub(visible_height)),
    };

    let end = (start + visible_height).min(total);

    let lines: Vec<Line> = buffer.lines()[start..end]
        .iter()
        .map(|line| {
            let style = if line.starts_with('[') {
                Style::default().fg(Color::Cyan)
            } else if line.contains("error") || line.contains("Error") {
                Style::default().fg(Color::Red)
            } else if line.contains("warning") || line.contains("Warning") {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            Line::from(Span::styled(line.as_str(), style))
        })
        .collect();

    let scroll_indicator = if total > visible_height {
        format!(" [{end}/{total}]")
    } else {
        String::new()
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
}
