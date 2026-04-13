use ratatui::{
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(frame: &mut Frame, area: ratatui::layout::Rect) {
    let content = Paragraph::new("Stories view — coming soon")
        .block(Block::default().borders(Borders::ALL).title("Stories"));
    frame.render_widget(content, area);
}
