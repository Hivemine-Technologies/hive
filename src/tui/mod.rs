pub mod tabs;
pub mod widgets;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::domain::{AgentEvent, IssueDetail, IssueFilters, OrchestratorEvent, StoryRun, TuiCommand};
use crate::trackers::IssueTracker;

use self::tabs::agents::{AgentFocus, AgentsState};
use self::tabs::config_tab::ConfigState;
use self::tabs::stories::StoriesState;
use self::tabs::worktrees::WorktreesState;
use self::tabs::Tab;

pub struct Tui {
    active_tab: Tab,
    runs: Vec<StoryRun>,
    should_quit: bool,
    show_help: bool,
    event_rx: mpsc::Receiver<OrchestratorEvent>,
    command_tx: mpsc::Sender<TuiCommand>,
    tracker: Arc<dyn IssueTracker>,
    tracker_config: crate::config::TrackerConfig,
    repo_path: PathBuf,
    config_dir: PathBuf,
    project_name: String,

    // Background detail fetch
    detail_rx: mpsc::Receiver<IssueDetail>,
    detail_tx: mpsc::Sender<IssueDetail>,

    // Per-tab state
    agents_state: AgentsState,
    stories_state: StoriesState,
    worktrees_state: WorktreesState,
    config_state: ConfigState,

    // Notification toast — (timestamp, message)
    notifications: Vec<(Instant, String)>,
}

impl Tui {
    pub fn new(
        event_rx: mpsc::Receiver<OrchestratorEvent>,
        command_tx: mpsc::Sender<TuiCommand>,
        tracker: Arc<dyn IssueTracker>,
        tracker_config: crate::config::TrackerConfig,
        repo_path: PathBuf,
        config_dir: PathBuf,
        project_name: String,
    ) -> Self {
        let (detail_tx, detail_rx) = mpsc::channel(16);
        Self {
            active_tab: Tab::Agents,
            runs: Vec::new(),
            should_quit: false,
            show_help: false,
            event_rx,
            command_tx,
            tracker,
            tracker_config,
            repo_path,
            config_dir,
            project_name,
            detail_rx,
            detail_tx,
            agents_state: AgentsState::new(),
            stories_state: StoriesState::new(),
            worktrees_state: WorktreesState::new(),
            config_state: ConfigState::new(),
            notifications: Vec::new(),
        }
    }

    fn notify(&mut self, msg: impl Into<String>) {
        self.notifications.push((Instant::now(), msg.into()));
    }

    /// Return the latest notification if it's less than 10 seconds old.
    fn active_notification(&self) -> Option<&str> {
        self.notifications
            .last()
            .filter(|(t, _)| t.elapsed() < Duration::from_secs(10))
            .map(|(_, msg)| msg.as_str())
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // Initial load for config tab
        self.config_state
            .load_config(&self.config_dir, &self.project_name);

        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if event::poll(Duration::from_millis(0))?
                        && let Event::Key(key) = event::read()? {
                        self.handle_key(key.code, key.modifiers).await;
                    }
                }
                Some(detail) = self.detail_rx.recv() => {
                    self.stories_state.set_detail(detail);
                }
                Some(event) = self.event_rx.recv() => {
                    self.handle_orchestrator_event(event);
                }
            }
        }

        Ok(())
    }

    fn render(&mut self, frame: &mut ratatui::Frame) {
        use ratatui::layout::{Constraint, Layout};

        let [tab_area, main_area, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        widgets::status_bar::render_tab_bar(frame, tab_area, &self.active_tab, &self.runs);

        match self.active_tab {
            Tab::Agents => {
                tabs::agents::render(frame, main_area, &self.runs, &mut self.agents_state);
            }
            Tab::Stories => {
                tabs::stories::render(frame, main_area, &self.stories_state);
            }
            Tab::Worktrees => {
                tabs::worktrees::render(frame, main_area, &self.worktrees_state, &self.runs);
            }
            Tab::Config => {
                tabs::config_tab::render(frame, main_area, &self.config_state);
            }
        }

        widgets::status_bar::render_status_bar(frame, status_area, self.active_notification());

        // Help overlay
        if self.show_help {
            self.render_help_overlay(frame);
        }
    }

    fn render_help_overlay(&self, frame: &mut ratatui::Frame) {
        use ratatui::layout::{Constraint, Layout};
        use ratatui::style::{Color, Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Clear, Paragraph};

        let area = frame.area();
        // Center a box roughly 60x24
        let [_, center_h, _] = Layout::vertical([
            Constraint::Fill(1),
            Constraint::Length(26),
            Constraint::Fill(1),
        ])
        .areas(area);
        let [_, popup, _] = Layout::horizontal([
            Constraint::Fill(1),
            Constraint::Length(60),
            Constraint::Fill(1),
        ])
        .areas(center_h);

        let help_text = match self.active_tab {
            Tab::Agents => vec![
                Line::from(Span::styled("Agents Tab", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(vec![Span::styled("j/k ", Style::default().fg(Color::Yellow)), Span::raw("Navigate agent list")]),
                Line::from(vec![Span::styled("Tab ", Style::default().fg(Color::Yellow)), Span::raw("Toggle sidebar / log panel focus")]),
                Line::from(vec![Span::styled("c   ", Style::default().fg(Color::Yellow)), Span::raw("Cancel selected agent")]),
                Line::from(vec![Span::styled("R   ", Style::default().fg(Color::Yellow)), Span::raw("Retry failed/attention agent")]),
                Line::from(vec![Span::styled("o   ", Style::default().fg(Color::Yellow)), Span::raw("Copy worktree path")]),
                Line::from(vec![Span::styled("g/G ", Style::default().fg(Color::Yellow)), Span::raw("Scroll log top/bottom")]),
            ],
            Tab::Stories => vec![
                Line::from(Span::styled("Stories Tab", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(vec![Span::styled("j/k   ", Style::default().fg(Color::Yellow)), Span::raw("Navigate stories")]),
                Line::from(vec![Span::styled("Enter ", Style::default().fg(Color::Yellow)), Span::raw("Start selected story")]),
                Line::from(vec![Span::styled("r     ", Style::default().fg(Color::Yellow)), Span::raw("Refresh story list")]),
                Line::from(vec![Span::styled("/     ", Style::default().fg(Color::Yellow)), Span::raw("Filter stories")]),
                Line::from(vec![Span::styled("s     ", Style::default().fg(Color::Yellow)), Span::raw("Cycle sort column")]),
                Line::from(vec![Span::styled("S     ", Style::default().fg(Color::Yellow)), Span::raw("Toggle sort direction")]),
            ],
            Tab::Worktrees => vec![
                Line::from(Span::styled("Worktrees Tab", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(vec![Span::styled("j/k ", Style::default().fg(Color::Yellow)), Span::raw("Navigate worktrees")]),
                Line::from(vec![Span::styled("r   ", Style::default().fg(Color::Yellow)), Span::raw("Refresh worktree list")]),
                Line::from(vec![Span::styled("d   ", Style::default().fg(Color::Yellow)), Span::raw("Delete selected worktree (y to confirm)")]),
                Line::from(vec![Span::styled("o   ", Style::default().fg(Color::Yellow)), Span::raw("Copy worktree path")]),
            ],
            Tab::Config => vec![
                Line::from(Span::styled("Config Tab", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
                Line::from(""),
                Line::from(vec![Span::styled("j/k ", Style::default().fg(Color::Yellow)), Span::raw("Scroll config")]),
                Line::from(vec![Span::styled("e   ", Style::default().fg(Color::Yellow)), Span::raw("Open config in $EDITOR")]),
                Line::from(vec![Span::styled("r   ", Style::default().fg(Color::Yellow)), Span::raw("Reload config from disk")]),
            ],
        };

        let mut lines = help_text;
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Global", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled("1-4 ", Style::default().fg(Color::Yellow)), Span::raw("Switch tabs")]));
        lines.push(Line::from(vec![Span::styled("q   ", Style::default().fg(Color::Yellow)), Span::raw("Quit (agents keep running)")]));
        lines.push(Line::from(vec![Span::styled("?   ", Style::default().fg(Color::Yellow)), Span::raw("Toggle this help")]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("Press any key to close", Style::default().fg(Color::DarkGray))));

        let paragraph = Paragraph::new(lines).block(
            Block::default()
                .title(" Help ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        );

        frame.render_widget(Clear, popup);
        frame.render_widget(paragraph, popup);
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // Dismiss help overlay on any key
        if self.show_help {
            self.show_help = false;
            return;
        }

        // Global keys
        match code {
            KeyCode::Char('?') if !self.stories_state.filter_active => {
                self.show_help = true;
                return;
            }
            KeyCode::Char('q') if !self.stories_state.filter_active => {
                let _ = self.command_tx.send(TuiCommand::Quit).await;
                self.should_quit = true;
                return;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.command_tx.send(TuiCommand::Quit).await;
                self.should_quit = true;
                return;
            }
            KeyCode::Char('1') if !self.stories_state.filter_active => {
                self.active_tab = Tab::Agents;
                return;
            }
            KeyCode::Char('2') if !self.stories_state.filter_active => {
                self.active_tab = Tab::Stories;
                self.fetch_stories_if_needed().await;
                return;
            }
            KeyCode::Char('3') if !self.stories_state.filter_active => {
                self.active_tab = Tab::Worktrees;
                self.worktrees_state.refresh(&self.repo_path);
                return;
            }
            KeyCode::Char('4') if !self.stories_state.filter_active => {
                self.active_tab = Tab::Config;
                return;
            }
            _ => {}
        }

        // Tab-specific keys
        match self.active_tab {
            Tab::Agents => self.handle_agents_key(code, modifiers).await,
            Tab::Stories => self.handle_stories_key(code, modifiers).await,
            Tab::Worktrees => self.handle_worktrees_key(code, modifiers).await,
            Tab::Config => self.handle_config_key(code).await,
        }
    }

    async fn handle_agents_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        let selected_issue_id = self
            .runs
            .get(self.agents_state.selected)
            .map(|r| r.issue_id.clone());

        match self.agents_state.focus {
            AgentFocus::Sidebar => match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if !self.runs.is_empty() {
                        self.agents_state.selected =
                            (self.agents_state.selected + 1).min(self.runs.len() - 1);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.agents_state.selected = self.agents_state.selected.saturating_sub(1);
                }
                KeyCode::Tab => self.agents_state.toggle_focus(),
                KeyCode::Char('c') => {
                    if let Some(id) = selected_issue_id {
                        let _ = self
                            .command_tx
                            .send(TuiCommand::CancelStory { issue_id: id })
                            .await;
                    }
                }
                KeyCode::Char('o') => {
                    if selected_issue_id.is_some() {
                        let _ = self
                            .command_tx
                            .send(TuiCommand::CopyWorktreePath)
                            .await;
                    }
                }
                KeyCode::Char('R') => {
                    if let Some(id) = selected_issue_id {
                        let _ = self
                            .command_tx
                            .send(TuiCommand::RetryStory { issue_id: id })
                            .await;
                    }
                }
                _ => {}
            },
            AgentFocus::LogPanel => match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        self.agents_state.cursor_down(id);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        self.agents_state.cursor_up(id);
                    }
                }
                KeyCode::Char('d') if modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        let h = self.agents_state.last_log_height as usize;
                        self.agents_state.half_page_down(id, h);
                        self.agents_state.snap_cursor_to_viewport(id);
                    }
                }
                KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        let h = self.agents_state.last_log_height as usize;
                        self.agents_state.half_page_up(id, h);
                        self.agents_state.snap_cursor_to_viewport(id);
                    }
                }
                KeyCode::PageDown => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        let h = self.agents_state.last_log_height as usize;
                        self.agents_state.page_down(id, h);
                        self.agents_state.snap_cursor_to_viewport(id);
                    }
                }
                KeyCode::PageUp => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        let h = self.agents_state.last_log_height as usize;
                        self.agents_state.page_up(id, h);
                        self.agents_state.snap_cursor_to_viewport(id);
                    }
                }
                KeyCode::Char('g') => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        self.agents_state.cursor_to_top(id);
                    }
                }
                KeyCode::Char('G') => {
                    if let Some(id) = selected_issue_id.as_ref() {
                        self.agents_state.cursor_to_bottom(id);
                    }
                }
                KeyCode::Tab => self.agents_state.toggle_focus(),
                _ => {}
            },
        }
    }

    async fn handle_stories_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if self.stories_state.filter_active {
            match code {
                KeyCode::Esc => self.stories_state.deactivate_filter(),
                KeyCode::Backspace => self.stories_state.filter_pop(),
                KeyCode::Char(c) => self.stories_state.filter_push(c),
                KeyCode::Enter => self.stories_state.deactivate_filter(),
                _ => {}
            }
            return;
        }

        use self::tabs::stories::StoriesFocus;
        match self.stories_state.focus {
            StoriesFocus::Table => match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    self.stories_state.move_down();
                    self.fetch_story_detail_if_needed();
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    self.stories_state.move_up();
                    self.fetch_story_detail_if_needed();
                }
                KeyCode::Tab => self.stories_state.toggle_focus(),
                KeyCode::Char('/') => self.stories_state.activate_filter(),
                KeyCode::Char('s') => self.stories_state.toggle_sort(),
                KeyCode::Char('S') => self.stories_state.toggle_sort_direction(),
                KeyCode::Char('r') => self.fetch_stories().await,
                KeyCode::Enter => {
                    if let Some(issue) = self.stories_state.selected_issue().cloned() {
                        let issue_id = issue.id.clone();
                        let _ = self
                            .command_tx
                            .send(TuiCommand::StartStory { issue })
                            .await;
                        // Remove the started story from the list
                        self.stories_state
                            .issues
                            .retain(|i| i.id != issue_id);
                        if self.stories_state.selected > 0
                            && self.stories_state.selected
                                >= self.stories_state.issues.len()
                        {
                            self.stories_state.selected =
                                self.stories_state.issues.len().saturating_sub(1);
                        }
                    }
                }
                _ => {}
            },
            StoriesFocus::Detail => match code {
                KeyCode::Char('j') | KeyCode::Down => self.stories_state.scroll_detail_down(),
                KeyCode::Char('k') | KeyCode::Up => self.stories_state.scroll_detail_up(),
                KeyCode::Tab => self.stories_state.toggle_focus(),
                _ => {}
            },
        }
    }

    async fn handle_worktrees_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if self.worktrees_state.confirm_delete {
            match code {
                KeyCode::Char('y') => {
                    // Actually delete the worktree
                    if let Some(wt) = self.worktrees_state.selected_worktree() {
                        let wt_path = wt.path.clone();
                        // Find the issue_id from the worktree path (last component)
                        let issue_id = wt_path
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("")
                            .to_string();
                        let worktree_dir = wt_path.parent().unwrap_or(&self.repo_path);
                        if let Err(e) = crate::git::worktree::remove_worktree(
                            &self.repo_path,
                            &issue_id,
                            worktree_dir,
                        ) {
                            tracing::warn!("Failed to delete worktree: {e}");
                            self.notify(format!("Failed to delete worktree: {e}"));
                        }
                    }
                    self.worktrees_state.confirm_delete = false;
                    self.worktrees_state.refresh(&self.repo_path);
                }
                _ => {
                    self.worktrees_state.confirm_delete = false;
                }
            }
            return;
        }

        match code {
            KeyCode::Char('j') | KeyCode::Down => self.worktrees_state.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.worktrees_state.move_up(),
            KeyCode::Char('r') => {
                // Refresh worktree list
                self.worktrees_state.refresh(&self.repo_path);
            }
            KeyCode::Char('d') => {
                self.worktrees_state.confirm_delete = true;
            }
            KeyCode::Char('o') => {
                if let Some(wt) = self.worktrees_state.selected_worktree() {
                    let path = wt.path.to_string_lossy().to_string();
                    let _ = std::process::Command::new("pbcopy")
                        .stdin(std::process::Stdio::piped())
                        .spawn()
                        .and_then(|mut child| {
                            use std::io::Write;
                            if let Some(stdin) = child.stdin.as_mut() {
                                let _ = stdin.write_all(path.as_bytes());
                            }
                            child.wait()
                        });
                }
            }
            _ => {}
        }
    }

    async fn handle_config_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('j') | KeyCode::Down => self.config_state.scroll_down(),
            KeyCode::Char('k') | KeyCode::Up => self.config_state.scroll_up(),
            KeyCode::Char('r') => {
                self.config_state
                    .load_config(&self.config_dir, &self.project_name);
            }
            KeyCode::Char('e') => {
                if let Some((editor, args)) = self.config_state.editor_command() {
                    ratatui::restore();
                    let _ = std::process::Command::new(&editor).args(&args).status();
                    self.config_state
                        .load_config(&self.config_dir, &self.project_name);
                }
            }
            _ => {}
        }
    }

    async fn fetch_stories_if_needed(&mut self) {
        if self.stories_state.issues.is_empty() && !self.stories_state.loading {
            self.fetch_stories().await;
        }
    }

    async fn fetch_stories(&mut self) {
        self.stories_state.loading = true;
        let filters = IssueFilters {
            team: Some(self.tracker_config.team.clone()),
            project: None,
            labels: vec![],
            statuses: self.tracker_config.ready_filter.clone(),
        };
        match self.tracker.list_ready(&filters).await {
            Ok(issues) => {
                self.stories_state.issues = issues;
                self.stories_state.loading = false;
                self.stories_state.invalidate_cache();
                self.fetch_story_detail_if_needed();
            }
            Err(e) => {
                tracing::warn!("Failed to fetch stories: {e}");
                self.notify(format!("Failed to fetch stories: {e}"));
                self.stories_state.loading = false;
            }
        }
    }

    fn fetch_story_detail_if_needed(&mut self) {
        if let Some(issue_id) = self.stories_state.needs_detail_fetch() {
            self.stories_state.detail_loading = true;
            let tracker = self.tracker.clone();
            let tx = self.detail_tx.clone();
            tokio::spawn(async move {
                match tracker.get_issue(&issue_id).await {
                    Ok(detail) => {
                        let _ = tx.send(detail).await;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to fetch story detail for {issue_id}: {e}");
                        // Note: can't call self.notify() from spawned task,
                        // but detail fetch failures are non-critical
                    }
                }
            });
        }
    }

    fn handle_orchestrator_event(&mut self, event: OrchestratorEvent) {
        match event {
            OrchestratorEvent::StoryUpdated(run) => {
                self.agents_state.ensure_buffer(&run.issue_id);

                if let Some(existing) = self.runs.iter_mut().find(|r| r.issue_id == run.issue_id) {
                    *existing = run;
                } else {
                    self.runs.push(run);
                }
            }
            OrchestratorEvent::AgentOutput { issue_id, event } => {
                use crate::tui::widgets::log_entry::{LogEntry, ToolResult};
                match event {
                    AgentEvent::TextDelta(text) => {
                        self.agents_state.append_entry(&issue_id, LogEntry::Text(text));
                    }
                    AgentEvent::ToolUse { tool_use_id, tool, input } => {
                        self.agents_state.append_entry(
                            &issue_id,
                            LogEntry::Tool {
                                tool_use_id,
                                tool,
                                input,
                                result: None,
                                started_at: std::time::Instant::now(),
                            },
                        );
                    }
                    AgentEvent::ToolResult { tool_use_id, output, is_error } => {
                        // Compute duration_ms from the matching ToolUse's started_at.
                        if let Some(buf) = self.agents_state.log_buffers.get(&issue_id) {
                            let now = std::time::Instant::now();
                            let duration_ms = buf
                                .entries()
                                .iter()
                                .rev()
                                .find_map(|e| match e {
                                    LogEntry::Tool {
                                        tool_use_id: id,
                                        started_at,
                                        ..
                                    } if id == &tool_use_id => {
                                        Some(now.duration_since(*started_at).as_millis() as u64)
                                    }
                                    _ => None,
                                })
                                .unwrap_or(0);
                            self.agents_state.attach_tool_result(
                                &issue_id,
                                &tool_use_id,
                                ToolResult {
                                    output,
                                    is_error,
                                    duration_ms,
                                },
                            );
                        }
                    }
                    AgentEvent::Error(msg) => {
                        self.notify(format!("{issue_id}: {msg}"));
                        self.agents_state
                            .append_entry(&issue_id, LogEntry::Text(format!("[error] {msg}")));
                    }
                    AgentEvent::Complete { cost_usd } => {
                        self.agents_state.append_entry(
                            &issue_id,
                            LogEntry::Marker(format!("complete · ${cost_usd:.4}")),
                        );
                    }
                }
            }
            OrchestratorEvent::PhaseTransition { issue_id, from, to } => {
                use crate::tui::widgets::log_entry::LogEntry;
                self.agents_state.append_entry(
                    &issue_id,
                    LogEntry::Marker(format!("Phase: {from} -> {to}")),
                );
            }
        }
    }
}
