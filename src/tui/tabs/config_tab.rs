use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

pub struct ConfigState {
    pub config_content: String,
    pub config_path: String,
    pub scroll: u16,
}

impl ConfigState {
    pub fn new() -> Self {
        Self {
            config_content: String::new(),
            config_path: String::new(),
            scroll: 0,
        }
    }

    pub fn load_config(&mut self, config_dir: &std::path::Path, project_name: &str) {
        let path = config_dir
            .join("projects")
            .join(project_name)
            .join("project.toml");
        self.config_path = path.to_string_lossy().to_string();

        match std::fs::read_to_string(&path) {
            Ok(content) => self.config_content = content,
            Err(e) => {
                self.config_content = format!("Error loading config: {e}");
            }
        }
        self.scroll = 0;
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    /// Open the config file in $EDITOR. Returns the editor command to run.
    /// The TUI should suspend before calling this.
    pub fn editor_command(&self) -> Option<(String, Vec<String>)> {
        let editor = std::env::var("EDITOR")
            .or_else(|_| std::env::var("VISUAL"))
            .unwrap_or_else(|_| "vim".to_string());

        if self.config_path.is_empty() {
            return None;
        }

        Some((editor, vec![self.config_path.clone()]))
    }
}

pub fn render(frame: &mut Frame, area: Rect, state: &ConfigState) {
    let [content_area, hint_area] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

    if state.config_content.is_empty() {
        let empty = Paragraph::new("No config loaded. Press 'r' to reload.")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Config"),
            );
        frame.render_widget(empty, content_area);
    } else {
        let lines: Vec<Line> = state
            .config_content
            .lines()
            .map(|line| {
                let style = if line.starts_with('[') {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else if line.starts_with('#') {
                    Style::default().fg(Color::DarkGray)
                } else if line.contains('=') {
                    let parts: Vec<&str> = line.splitn(2, '=').collect();
                    if parts.len() == 2 {
                        return Line::from(vec![
                            Span::styled(
                                parts[0].to_string(),
                                Style::default().fg(Color::Yellow),
                            ),
                            Span::raw("="),
                            Span::styled(
                                parts[1].to_string(),
                                Style::default().fg(Color::Green),
                            ),
                        ]);
                    }
                    Style::default()
                } else {
                    Style::default()
                };
                Line::from(Span::styled(line.to_string(), style))
            })
            .collect();

        let title = format!("Config - {}", state.config_path);
        let content = Paragraph::new(lines)
            .block(Block::default().borders(Borders::ALL).title(title))
            .scroll((state.scroll, 0))
            .wrap(Wrap { trim: false });

        frame.render_widget(content, content_area);
    }

    // Hints
    let hints = Line::from(vec![
        Span::styled("e", Style::default().fg(Color::Cyan)),
        Span::raw(" edit in $EDITOR  "),
        Span::styled("r", Style::default().fg(Color::Cyan)),
        Span::raw(" reload  "),
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" scroll"),
    ]);
    let hint_bar = Paragraph::new(hints);
    frame.render_widget(hint_bar, hint_area);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_state_new() {
        let state = ConfigState::new();
        assert!(state.config_content.is_empty());
        assert!(state.config_path.is_empty());
        assert_eq!(state.scroll, 0);
    }

    #[test]
    fn test_scroll() {
        let mut state = ConfigState::new();
        state.scroll_down();
        assert_eq!(state.scroll, 1);
        state.scroll_down();
        assert_eq!(state.scroll, 2);
        state.scroll_up();
        assert_eq!(state.scroll, 1);
        state.scroll_up();
        assert_eq!(state.scroll, 0);
        state.scroll_up();
        assert_eq!(state.scroll, 0); // clamps
    }

    #[test]
    fn test_editor_command_default() {
        let mut state = ConfigState::new();
        state.config_path = "/some/path.toml".to_string();
        let cmd = state.editor_command();
        assert!(cmd.is_some());
        let (_, args) = cmd.unwrap();
        assert_eq!(args, vec!["/some/path.toml"]);
    }

    #[test]
    fn test_editor_command_empty_path() {
        let state = ConfigState::new();
        assert!(state.editor_command().is_none());
    }
}
