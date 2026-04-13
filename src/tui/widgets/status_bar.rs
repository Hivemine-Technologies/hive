use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::domain::{RunStatus, StoryRun};

use crate::tui::tabs::Tab;

pub fn render_tab_bar(frame: &mut Frame, area: Rect, active_tab: &Tab, runs: &[StoryRun]) {
    let mut spans: Vec<Span> = Vec::new();

    for tab in Tab::all() {
        let label = format!("[{}] {} ", tab.index() + 1, tab.label());
        let style = if tab == active_tab {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(label, style));
    }

    // Right-side stats
    let running = runs
        .iter()
        .filter(|r| r.status == RunStatus::Running)
        .count();
    let attention = runs
        .iter()
        .filter(|r| r.status == RunStatus::NeedsAttention)
        .count();
    let total_cost: f64 = runs.iter().map(|r| r.cost_usd).sum();

    let mut right_spans: Vec<Span> = Vec::new();
    if running > 0 {
        right_spans.push(Span::styled(
            format!("\u{25cf} {running} running  "),
            Style::default().fg(Color::Green),
        ));
    }
    if attention > 0 {
        right_spans.push(Span::styled(
            format!("\u{25cf} {attention} attn  "),
            Style::default().fg(Color::Yellow),
        ));
    }
    right_spans.push(Span::styled(
        format!("${total_cost:.2}"),
        Style::default().fg(Color::Gray),
    ));

    // Calculate padding to right-align stats
    let left_len: usize = spans.iter().map(|s| s.content.len()).sum();
    let right_len: usize = right_spans.iter().map(|s| s.content.len()).sum();
    let padding = (area.width as usize).saturating_sub(left_len + right_len);
    spans.push(Span::raw(" ".repeat(padding)));
    spans.extend(right_spans);

    let tab_bar = Paragraph::new(Line::from(spans));
    frame.render_widget(tab_bar, area);
}

pub fn render_status_bar(frame: &mut Frame, area: Rect, _runs: &[StoryRun]) {
    let hints = Line::from(vec![
        Span::styled("q", Style::default().fg(Color::Cyan)),
        Span::raw(" quit  "),
        Span::styled("Tab", Style::default().fg(Color::Cyan)),
        Span::raw(" switch  "),
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("?", Style::default().fg(Color::Cyan)),
        Span::raw(" help"),
    ]);

    let bar = Paragraph::new(hints);
    frame.render_widget(bar, area);
}
