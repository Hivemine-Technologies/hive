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
}
