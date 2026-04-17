use ratatui::{
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
    Frame,
};

use crate::domain::StoryRun;
use crate::git::worktree::WorktreeInfo;

pub struct WorktreesState {
    pub worktrees: Vec<WorktreeInfo>,
    pub selected: usize,
    pub confirm_delete: bool,
}

impl WorktreesState {
    pub fn new() -> Self {
        Self {
            worktrees: Vec::new(),
            selected: 0,
            confirm_delete: false,
        }
    }

    pub fn refresh(&mut self, repo_path: &std::path::Path) {
        match crate::git::worktree::list_worktrees(repo_path) {
            Ok(wts) => {
                self.worktrees = wts;
                if self.selected >= self.worktrees.len() && !self.worktrees.is_empty() {
                    self.selected = self.worktrees.len() - 1;
                }
            }
            Err(e) => {
                tracing::warn!("Failed to list worktrees: {e}");
            }
        }
    }

    pub fn move_down(&mut self) {
        let max = self.worktrees.len().saturating_sub(1);
        self.selected = (self.selected + 1).min(max);
    }

    pub fn move_up(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn selected_worktree(&self) -> Option<&WorktreeInfo> {
        self.worktrees.get(self.selected)
    }
}

fn worktree_status(wt: &WorktreeInfo, runs: &[StoryRun]) -> (&'static str, Color) {
    if wt.is_bare {
        return ("bare", Color::DarkGray);
    }
    if let Some(ref branch) = wt.branch {
        // Check if any run is using this branch
        for run in runs {
            if let Some(ref run_branch) = run.branch
                && run_branch == branch {
                return match run.status {
                    crate::domain::RunStatus::Running => ("running", Color::Green),
                    crate::domain::RunStatus::NeedsAttention => ("attn", Color::Yellow),
                    crate::domain::RunStatus::Complete => ("done", Color::Blue),
                    crate::domain::RunStatus::Paused => ("paused", Color::Gray),
                    crate::domain::RunStatus::Failed => ("failed", Color::Red),
                };
            }
        }
    }
    ("idle", Color::DarkGray)
}

pub fn render(frame: &mut Frame, area: Rect, state: &WorktreesState, runs: &[StoryRun]) {
    if state.worktrees.is_empty() {
        let empty = Paragraph::new("No worktrees found. Start a story to create one.")
            .style(Style::default().fg(Color::DarkGray))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Worktrees"),
            );
        frame.render_widget(empty, area);
        return;
    }

    let headers = Row::new(vec![
        Cell::from("Branch"),
        Cell::from("Path"),
        Cell::from("Status"),
    ])
    .style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let rows: Vec<Row> = state
        .worktrees
        .iter()
        .enumerate()
        .map(|(i, wt)| {
            let style = if i == state.selected {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            let branch_str = wt
                .branch
                .as_deref()
                .unwrap_or("(detached)")
                .to_string();
            let path_str = wt.path.to_string_lossy().to_string();
            let (status_str, status_color) = worktree_status(wt, runs);
            Row::new(vec![
                Cell::from(branch_str),
                Cell::from(path_str),
                Cell::from(status_str.to_string())
                    .style(Style::default().fg(status_color)),
            ])
            .style(style)
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Percentage(30),
            Constraint::Percentage(50),
            Constraint::Percentage(20),
        ],
    )
    .header(headers)
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(format!("Worktrees ({})", state.worktrees.len())),
    );

    frame.render_widget(table, area);

    // Confirmation overlay
    if state.confirm_delete
        && let Some(wt) = state.selected_worktree() {
        let branch = wt.branch.as_deref().unwrap_or("unknown");
        let msg = Line::from(vec![
            Span::styled("Delete worktree ", Style::default().fg(Color::Yellow)),
            Span::styled(
                branch,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("? (y/n)", Style::default().fg(Color::Yellow)),
        ]);
        let confirm = Paragraph::new(msg);
        let confirm_area = Rect {
            x: area.x + 1,
            y: area.y + area.height.saturating_sub(2),
            width: area.width.saturating_sub(2),
            height: 1,
        };
        frame.render_widget(confirm, confirm_area);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_worktrees_state_new() {
        let state = WorktreesState::new();
        assert_eq!(state.selected, 0);
        assert!(state.worktrees.is_empty());
        assert!(!state.confirm_delete);
    }

    #[test]
    fn test_move_down_clamps() {
        let mut state = WorktreesState::new();
        state.worktrees = vec![
            WorktreeInfo {
                path: PathBuf::from("/a"),
                branch: Some("main".to_string()),
                is_bare: false,
            },
            WorktreeInfo {
                path: PathBuf::from("/b"),
                branch: Some("feat".to_string()),
                is_bare: false,
            },
        ];
        state.move_down();
        assert_eq!(state.selected, 1);
        state.move_down();
        assert_eq!(state.selected, 1);
    }

    #[test]
    fn test_move_up_clamps() {
        let mut state = WorktreesState::new();
        state.move_up();
        assert_eq!(state.selected, 0);
    }

    #[test]
    fn test_selected_worktree() {
        let mut state = WorktreesState::new();
        assert!(state.selected_worktree().is_none());
        state.worktrees = vec![WorktreeInfo {
            path: PathBuf::from("/a"),
            branch: Some("main".to_string()),
            is_bare: false,
        }];
        assert!(state.selected_worktree().is_some());
    }

    #[test]
    fn test_worktree_status_bare() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/bare"),
            branch: None,
            is_bare: true,
        };
        let (status, _) = worktree_status(&wt, &[]);
        assert_eq!(status, "bare");
    }

    #[test]
    fn test_worktree_status_idle() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/idle"),
            branch: Some("feature-1".to_string()),
            is_bare: false,
        };
        let (status, _) = worktree_status(&wt, &[]);
        assert_eq!(status, "idle");
    }

    #[test]
    fn test_worktree_status_running() {
        let wt = WorktreeInfo {
            path: PathBuf::from("/running"),
            branch: Some("APX-1/feature".to_string()),
            is_bare: false,
        };
        let mut run = StoryRun::new("APX-1".to_string(), "Test".to_string());
        run.branch = Some("APX-1/feature".to_string());
        run.status = crate::domain::RunStatus::Running;
        let (status, _) = worktree_status(&wt, &[run]);
        assert_eq!(status, "running");
    }
}
