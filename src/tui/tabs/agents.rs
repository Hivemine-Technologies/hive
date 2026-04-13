use ratatui::{
    layout::{Constraint, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::domain::{RunStatus, StoryRun};

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

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect, runs: &[StoryRun], selected: usize) {
    let [sidebar_area, main_area] =
        Layout::horizontal([Constraint::Percentage(30), Constraint::Percentage(70)])
            .areas(area);

    // Sidebar: list of agents
    let items: Vec<ListItem> = runs
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let color = status_color(&run.status);
            let icon = status_icon(&run.status);
            let style = if i == selected {
                Style::default().fg(color).bg(Color::DarkGray)
            } else {
                Style::default().fg(color)
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{icon} "), style),
                Span::styled(&run.issue_id, style),
            ]))
        })
        .collect();

    let sidebar = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("Agents"));

    frame.render_widget(sidebar, sidebar_area);

    // Main panel: selected agent details
    let detail = if let Some(run) = runs.get(selected) {
        let lines = vec![
            Line::from(vec![
                Span::styled("Issue:  ", Style::default().fg(Color::DarkGray)),
                Span::raw(&run.issue_id),
            ]),
            Line::from(vec![
                Span::styled("Title:  ", Style::default().fg(Color::DarkGray)),
                Span::raw(&run.issue_title),
            ]),
            Line::from(vec![
                Span::styled("Phase:  ", Style::default().fg(Color::DarkGray)),
                Span::raw(run.phase.to_string()),
            ]),
            Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{} {:?}", status_icon(&run.status), run.status),
                    Style::default().fg(status_color(&run.status)),
                ),
            ]),
            Line::from(vec![
                Span::styled("Cost:   ", Style::default().fg(Color::DarkGray)),
                Span::raw(format!("${:.2}", run.cost_usd)),
            ]),
        ];
        Paragraph::new(lines)
    } else {
        Paragraph::new("No agent selected")
    };

    let detail_block = detail.block(Block::default().borders(Borders::ALL).title("Details"));
    frame.render_widget(detail_block, main_area);
}
