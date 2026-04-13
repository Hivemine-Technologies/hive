use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::domain::Issue;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortColumn {
    Id,
    Title,
    Priority,
    Project,
}

impl SortColumn {
    pub fn next(&self) -> Self {
        match self {
            SortColumn::Id => SortColumn::Title,
            SortColumn::Title => SortColumn::Priority,
            SortColumn::Priority => SortColumn::Project,
            SortColumn::Project => SortColumn::Id,
        }
    }
}

pub struct StoriesState {
    pub issues: Vec<Issue>,
    pub selected: usize,
    pub filter_text: String,
    pub sort_column: SortColumn,
    pub sort_ascending: bool,
    pub loading: bool,
    pub filter_active: bool,
}

impl StoriesState {
    pub fn new() -> Self {
        Self {
            issues: Vec::new(),
            selected: 0,
            filter_text: String::new(),
            sort_column: SortColumn::Priority,
            sort_ascending: true,
            loading: false,
            filter_active: false,
        }
    }

    pub fn filtered_issues(&self) -> Vec<&Issue> {
        let mut issues: Vec<&Issue> = if self.filter_text.is_empty() {
            self.issues.iter().collect()
        } else {
            let filter = self.filter_text.to_lowercase();
            self.issues
                .iter()
                .filter(|i| {
                    i.id.to_lowercase().contains(&filter)
                        || i.title.to_lowercase().contains(&filter)
                })
                .collect()
        };

        issues.sort_by(|a, b| {
            let cmp = match self.sort_column {
                SortColumn::Id => a.id.cmp(&b.id),
                SortColumn::Title => a.title.cmp(&b.title),
                SortColumn::Priority => {
                    priority_rank(a.priority.as_deref())
                        .cmp(&priority_rank(b.priority.as_deref()))
                }
                SortColumn::Project => a
                    .project
                    .as_deref()
                    .unwrap_or("")
                    .cmp(b.project.as_deref().unwrap_or("")),
            };
            if self.sort_ascending {
                cmp
            } else {
                cmp.reverse()
            }
        });

        issues
    }

    pub fn move_down(&mut self) {
        let max = self.filtered_issues().len().saturating_sub(1);
        self.selected = (self.selected + 1).min(max);
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn toggle_sort(&mut self) {
        self.sort_column = self.sort_column.next();
    }

    pub fn toggle_sort_direction(&mut self) {
        self.sort_ascending = !self.sort_ascending;
    }

    pub fn selected_issue(&self) -> Option<&Issue> {
        let filtered = self.filtered_issues();
        filtered.get(self.selected).copied()
    }

    pub fn activate_filter(&mut self) {
        self.filter_active = true;
    }

    pub fn deactivate_filter(&mut self) {
        self.filter_active = false;
        self.filter_text.clear();
        self.selected = 0;
    }

    pub fn filter_push(&mut self, c: char) {
        self.filter_text.push(c);
        self.selected = 0;
    }

    pub fn filter_pop(&mut self) {
        self.filter_text.pop();
        self.selected = 0;
    }
}

fn priority_rank(p: Option<&str>) -> u8 {
    match p {
        Some("Urgent") => 0,
        Some("High") => 1,
        Some("Medium") => 2,
        Some("Low") => 3,
        _ => 4,
    }
}

fn priority_color(p: Option<&str>) -> Color {
    match p {
        Some("Urgent") => Color::Red,
        Some("High") => Color::LightRed,
        Some("Medium") => Color::Yellow,
        Some("Low") => Color::Gray,
        _ => Color::DarkGray,
    }
}

pub fn render(frame: &mut Frame, area: Rect, state: &StoriesState) {
    let [table_area, filter_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(if state.filter_active { 1 } else { 0 }),
    ])
    .areas(area);

    if state.loading {
        let loading = Paragraph::new("Loading stories...")
            .style(Style::default().fg(Color::Yellow))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Stories"),
            );
        frame.render_widget(loading, table_area);
        return;
    }

    let filtered = state.filtered_issues();

    if filtered.is_empty() {
        let msg = if state.issues.is_empty() {
            "No stories loaded. Press 'r' to fetch from tracker."
        } else {
            "No stories match filter."
        };
        let empty = Paragraph::new(msg)
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Stories"),
            );
        frame.render_widget(empty, table_area);
        return;
    }

    // Column headers with sort indicator
    let sort_indicator = if state.sort_ascending { " ^" } else { " v" };
    let headers = Row::new(vec![
        Cell::from(format!(
            "ID{}",
            if state.sort_column == SortColumn::Id {
                sort_indicator
            } else {
                ""
            }
        )),
        Cell::from(format!(
            "Title{}",
            if state.sort_column == SortColumn::Title {
                sort_indicator
            } else {
                ""
            }
        )),
        Cell::from(format!(
            "Priority{}",
            if state.sort_column == SortColumn::Priority {
                sort_indicator
            } else {
                ""
            }
        )),
        Cell::from(format!(
            "Project{}",
            if state.sort_column == SortColumn::Project {
                sort_indicator
            } else {
                ""
            }
        )),
        Cell::from("Labels"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = filtered
        .iter()
        .enumerate()
        .map(|(i, issue)| {
            let style = if i == state.selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            let priority_str = issue.priority.as_deref().unwrap_or("None");
            let labels_str = issue.labels.join(", ");
            let title = if issue.title.len() > 50 {
                format!("{}...", &issue.title[..47])
            } else {
                issue.title.clone()
            };
            Row::new(vec![
                Cell::from(issue.id.clone()),
                Cell::from(title),
                Cell::from(priority_str.to_string())
                    .style(Style::default().fg(priority_color(issue.priority.as_deref()))),
                Cell::from(
                    issue
                        .project
                        .as_deref()
                        .unwrap_or("-")
                        .to_string(),
                ),
                Cell::from(labels_str),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(12),
            Constraint::Fill(1),
            Constraint::Length(10),
            Constraint::Length(15),
            Constraint::Length(20),
        ],
    )
    .header(headers)
    .block(
        Block::default().borders(Borders::ALL).title(format!(
            "Stories ({}/{})",
            filtered.len(),
            state.issues.len()
        )),
    );

    frame.render_widget(table, table_area);

    // Filter bar
    if state.filter_active {
        let filter_line = Line::from(vec![
            Span::styled("/", Style::default().fg(Color::Cyan)),
            Span::raw(&state.filter_text),
            Span::styled("_", Style::default().fg(Color::Gray)),
        ]);
        let filter_bar = Paragraph::new(filter_line);
        frame.render_widget(filter_bar, filter_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Issue;

    fn make_issue(id: &str, title: &str, priority: &str) -> Issue {
        Issue {
            id: id.to_string(),
            title: title.to_string(),
            priority: Some(priority.to_string()),
            project: Some("TestProject".to_string()),
            labels: vec![],
            url: String::new(),
        }
    }

    #[test]
    fn test_stories_state_new() {
        let state = StoriesState::new();
        assert_eq!(state.selected, 0);
        assert!(state.issues.is_empty());
        assert!(!state.loading);
        assert!(!state.filter_active);
    }

    #[test]
    fn test_filter_by_title() {
        let mut state = StoriesState::new();
        state.issues = vec![
            make_issue("APX-1", "Add auth", "High"),
            make_issue("APX-2", "Fix bug", "Low"),
            make_issue("APX-3", "Add logging", "Medium"),
        ];
        state.filter_text = "add".to_string();
        let filtered = state.filtered_issues();
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn test_filter_by_id() {
        let mut state = StoriesState::new();
        state.issues = vec![
            make_issue("APX-1", "Story one", "High"),
            make_issue("APX-2", "Story two", "Low"),
        ];
        state.filter_text = "APX-2".to_string();
        let filtered = state.filtered_issues();
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "APX-2");
    }

    #[test]
    fn test_sort_by_priority() {
        let mut state = StoriesState::new();
        state.sort_column = SortColumn::Priority;
        state.sort_ascending = true;
        state.issues = vec![
            make_issue("APX-1", "A", "Low"),
            make_issue("APX-2", "B", "Urgent"),
            make_issue("APX-3", "C", "Medium"),
        ];
        let filtered = state.filtered_issues();
        assert_eq!(filtered[0].id, "APX-2"); // Urgent
        assert_eq!(filtered[1].id, "APX-3"); // Medium
        assert_eq!(filtered[2].id, "APX-1"); // Low
    }

    #[test]
    fn test_sort_descending() {
        let mut state = StoriesState::new();
        state.sort_column = SortColumn::Id;
        state.sort_ascending = false;
        state.issues = vec![
            make_issue("APX-1", "A", "High"),
            make_issue("APX-3", "C", "Low"),
            make_issue("APX-2", "B", "Medium"),
        ];
        let filtered = state.filtered_issues();
        assert_eq!(filtered[0].id, "APX-3");
        assert_eq!(filtered[2].id, "APX-1");
    }

    #[test]
    fn test_move_down_clamps() {
        let mut state = StoriesState::new();
        state.issues = vec![
            make_issue("APX-1", "A", "High"),
            make_issue("APX-2", "B", "Low"),
        ];
        state.move_down();
        assert_eq!(state.selected, 1);
        state.move_down();
        assert_eq!(state.selected, 1); // clamped
    }

    #[test]
    fn test_move_up_clamps() {
        let mut state = StoriesState::new();
        state.move_up();
        assert_eq!(state.selected, 0); // doesn't go negative
    }

    #[test]
    fn test_toggle_sort_cycles() {
        let mut state = StoriesState::new();
        state.sort_column = SortColumn::Id;
        state.toggle_sort();
        assert_eq!(state.sort_column, SortColumn::Title);
        state.toggle_sort();
        assert_eq!(state.sort_column, SortColumn::Priority);
        state.toggle_sort();
        assert_eq!(state.sort_column, SortColumn::Project);
        state.toggle_sort();
        assert_eq!(state.sort_column, SortColumn::Id);
    }

    #[test]
    fn test_filter_activation() {
        let mut state = StoriesState::new();
        state.activate_filter();
        assert!(state.filter_active);
        state.filter_push('a');
        state.filter_push('b');
        assert_eq!(state.filter_text, "ab");
        state.filter_pop();
        assert_eq!(state.filter_text, "a");
        state.deactivate_filter();
        assert!(!state.filter_active);
        assert!(state.filter_text.is_empty());
    }

    #[test]
    fn test_selected_issue() {
        let mut state = StoriesState::new();
        state.issues = vec![
            make_issue("APX-1", "A", "High"),
            make_issue("APX-2", "B", "Low"),
        ];
        assert_eq!(state.selected_issue().unwrap().id, "APX-1"); // sorted by priority: High first
        state.selected = 1;
        assert_eq!(state.selected_issue().unwrap().id, "APX-2");
    }

    #[test]
    fn test_priority_rank_ordering() {
        assert!(priority_rank(Some("Urgent")) < priority_rank(Some("High")));
        assert!(priority_rank(Some("High")) < priority_rank(Some("Medium")));
        assert!(priority_rank(Some("Medium")) < priority_rank(Some("Low")));
        assert!(priority_rank(Some("Low")) < priority_rank(None));
    }
}
