use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::domain::{RunStatus, StoryRun};
use crate::tui::widgets::{log_viewer, phase_bar};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentFocus {
    Sidebar,
    LogPanel,
}

pub use crate::tui::widgets::log_viewer::ScrollPos;

pub struct AgentsState {
    pub selected: usize,
    pub focus: AgentFocus,
    pub log_buffers: HashMap<String, log_viewer::LogBuffer>,
    pub log_scroll: HashMap<String, ScrollPos>,
}

impl AgentsState {
    pub fn new() -> Self {
        Self {
            selected: 0,
            focus: AgentFocus::Sidebar,
            log_buffers: HashMap::new(),
            log_scroll: HashMap::new(),
        }
    }

    pub fn ensure_buffer(&mut self, issue_id: &str) {
        self.log_buffers
            .entry(issue_id.to_string())
            .or_insert_with(|| log_viewer::LogBuffer::new(5000));
        self.log_scroll
            .entry(issue_id.to_string())
            .or_default();
    }

    pub fn append_log(&mut self, issue_id: &str, line: String) {
        self.ensure_buffer(issue_id);
        if let Some(buf) = self.log_buffers.get_mut(issue_id) {
            buf.push(line);
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            AgentFocus::Sidebar => AgentFocus::LogPanel,
            AgentFocus::LogPanel => AgentFocus::Sidebar,
        };
    }

    pub fn scroll_log_down(&mut self, issue_id: &str) {
        let Some(pos) = self.log_scroll.get_mut(issue_id) else { return };
        let Some(buf) = self.log_buffers.get(issue_id) else { return };
        // When already at Tail, there's nothing below to scroll to.
        if let ScrollPos::Offset(n) = *pos {
            let next = n + 1;
            // Snap to Tail if we've caught up — better than "stuck one line below tail".
            if next + 1 >= buf.len() {
                *pos = ScrollPos::Tail;
            } else {
                *pos = ScrollPos::Offset(next);
            }
        }
    }

    pub fn scroll_log_up(&mut self, issue_id: &str) {
        let Some(pos) = self.log_scroll.get_mut(issue_id) else { return };
        let Some(buf) = self.log_buffers.get(issue_id) else { return };
        match *pos {
            ScrollPos::Tail => {
                // Break out of follow mode at the penultimate line.
                if buf.len() > 1 {
                    *pos = ScrollPos::Offset(buf.len() - 2);
                }
            }
            ScrollPos::Offset(0) => { /* already at top */ }
            ScrollPos::Offset(n) => *pos = ScrollPos::Offset(n - 1),
        }
    }

    pub fn scroll_to_top(&mut self, issue_id: &str) {
        if let Some(pos) = self.log_scroll.get_mut(issue_id) {
            *pos = ScrollPos::Offset(0);
        }
    }

    pub fn scroll_to_bottom(&mut self, issue_id: &str) {
        if let Some(pos) = self.log_scroll.get_mut(issue_id) {
            *pos = ScrollPos::Tail;
        }
    }
}

fn status_color(status: &RunStatus) -> Color {
    match status {
        RunStatus::Running => Color::Green,
        RunStatus::NeedsAttention => Color::Yellow,
        RunStatus::Complete => Color::Blue,
        RunStatus::Paused => Color::Gray,
        RunStatus::Failed => Color::Red,
    }
}

fn status_icon(status: &RunStatus) -> &'static str {
    match status {
        RunStatus::Running => "\u{25b6}",
        RunStatus::NeedsAttention => "\u{26a0}",
        RunStatus::Complete => "\u{2713}",
        RunStatus::Paused => "\u{23f8}",
        RunStatus::Failed => "\u{2717}",
    }
}

pub fn render(
    frame: &mut Frame,
    area: Rect,
    runs: &[StoryRun],
    state: &AgentsState,
) {
    let [sidebar_area, main_area] =
        Layout::horizontal([Constraint::Percentage(25), Constraint::Percentage(75)])
            .areas(area);

    // Sidebar: list of agents
    let items: Vec<ListItem> = runs
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let color = status_color(&run.status);
            let icon = status_icon(&run.status);
            let style = if i == state.selected {
                Style::default()
                    .fg(color)
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), style),
                Span::styled(&run.issue_id, style),
                Span::styled(
                    format!(" {}", run.phase.config_key()),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();

    let sidebar_style = if state.focus == AgentFocus::Sidebar {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let sidebar = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Agents")
            .border_style(sidebar_style),
    );

    frame.render_widget(sidebar, sidebar_area);

    // Main panel
    if let Some(run) = runs.get(state.selected) {
        let [header_area, phase_area, log_area, hint_area] = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(main_area);

        // Header
        let header_lines = vec![
            Line::from(vec![
                Span::styled("Issue: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&run.issue_id),
                Span::styled("  Title: ", Style::default().fg(Color::DarkGray)),
                Span::raw(&run.issue_title),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} {:?}", status_icon(&run.status), run.status),
                    Style::default().fg(status_color(&run.status)),
                ),
                Span::styled("  Cost: ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("${:.2}", run.cost_usd)),
            ]),
        ];
        let header =
            Paragraph::new(header_lines).block(Block::default().borders(Borders::BOTTOM));
        frame.render_widget(header, header_area);

        // Phase bar
        phase_bar::render_phase_bar(frame, phase_area, &run.phase);

        // Log viewer
        let _log_style = if state.focus == AgentFocus::LogPanel {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let scroll = state
            .log_scroll
            .get(&run.issue_id)
            .copied()
            .unwrap_or(ScrollPos::Tail);

        if let Some(buffer) = state.log_buffers.get(&run.issue_id) {
            log_viewer::render_log(frame, log_area, buffer, scroll, "Output");
        } else {
            let empty = Paragraph::new("No output yet.")
                .style(Style::default().fg(Color::DarkGray))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("Output"),
                );
            frame.render_widget(empty, log_area);
        }

        // Hints
        let hints = Line::from(vec![
            Span::styled("Tab", Style::default().fg(Color::Cyan)),
            Span::raw(" focus  "),
            Span::styled("g/G", Style::default().fg(Color::Cyan)),
            Span::raw(" top/bottom  "),
            Span::styled("c", Style::default().fg(Color::Cyan)),
            Span::raw(" cancel  "),
            Span::styled("o", Style::default().fg(Color::Cyan)),
            Span::raw(" copy path"),
        ]);
        let hint_bar = Paragraph::new(hints);
        frame.render_widget(hint_bar, hint_area);
    } else {
        let empty = Paragraph::new("No agents running. Start a story from the Stories tab.")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL).title("Details"));
        frame.render_widget(empty, main_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agents_state_new() {
        let state = AgentsState::new();
        assert_eq!(state.selected, 0);
        assert_eq!(state.focus, AgentFocus::Sidebar);
        assert!(state.log_buffers.is_empty());
    }

    #[test]
    fn test_ensure_buffer_creates_entry() {
        let mut state = AgentsState::new();
        state.ensure_buffer("APX-1");
        assert!(state.log_buffers.contains_key("APX-1"));
        assert!(state.log_scroll.contains_key("APX-1"));
    }

    #[test]
    fn test_append_log() {
        let mut state = AgentsState::new();
        state.append_log("APX-1", "hello".to_string());
        assert_eq!(state.log_buffers["APX-1"].len(), 1);
    }

    #[test]
    fn test_toggle_focus() {
        let mut state = AgentsState::new();
        assert_eq!(state.focus, AgentFocus::Sidebar);
        state.toggle_focus();
        assert_eq!(state.focus, AgentFocus::LogPanel);
        state.toggle_focus();
        assert_eq!(state.focus, AgentFocus::Sidebar);
    }

    #[test]
    fn test_scroll_to_top_and_bottom() {
        let mut state = AgentsState::new();
        state.ensure_buffer("APX-1");
        for i in 0..100 {
            state.append_log("APX-1", format!("line {i}"));
        }
        state.scroll_to_top("APX-1");
        assert_eq!(state.log_scroll["APX-1"], ScrollPos::Offset(0));
        state.scroll_to_bottom("APX-1");
        assert_eq!(state.log_scroll["APX-1"], ScrollPos::Tail);
    }

    #[test]
    fn test_scroll_down_snaps_to_tail_at_end() {
        let mut state = AgentsState::new();
        state.ensure_buffer("APX-1");
        for i in 0..10 {
            state.append_log("APX-1", format!("line {i}"));
        }
        // Move into manual mode one step from the bottom.
        state.log_scroll.insert("APX-1".to_string(), ScrollPos::Offset(8));
        state.scroll_log_down("APX-1");
        // `next + 1 >= buf.len()` is true at Offset(9) so we snap back to Tail.
        assert_eq!(state.log_scroll["APX-1"], ScrollPos::Tail);
    }

    #[test]
    fn test_scroll_up_from_tail_enters_manual() {
        let mut state = AgentsState::new();
        state.ensure_buffer("APX-1");
        for i in 0..5 {
            state.append_log("APX-1", format!("line {i}"));
        }
        assert_eq!(state.log_scroll["APX-1"], ScrollPos::Tail);
        state.scroll_log_up("APX-1");
        assert_eq!(state.log_scroll["APX-1"], ScrollPos::Offset(3));
    }

    #[test]
    fn test_scroll_up_at_top_is_noop() {
        let mut state = AgentsState::new();
        state.ensure_buffer("APX-1");
        for i in 0..5 { state.append_log("APX-1", format!("line {i}")); }
        state.scroll_to_top("APX-1");
        assert_eq!(state.log_scroll["APX-1"], ScrollPos::Offset(0));
        state.scroll_log_up("APX-1");
        assert_eq!(state.log_scroll["APX-1"], ScrollPos::Offset(0));
    }
}
