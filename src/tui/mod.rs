pub mod tabs;
pub mod widgets;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::domain::{AgentEvent, IssueFilters, OrchestratorEvent, StoryRun, TuiCommand};
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
    event_rx: mpsc::Receiver<OrchestratorEvent>,
    command_tx: mpsc::Sender<TuiCommand>,
    tracker: Arc<dyn IssueTracker>,
    tracker_config: crate::config::TrackerConfig,
    repo_path: PathBuf,
    config_dir: PathBuf,
    project_name: String,

    // Per-tab state
    agents_state: AgentsState,
    stories_state: StoriesState,
    worktrees_state: WorktreesState,
    config_state: ConfigState,
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
        Self {
            active_tab: Tab::Agents,
            runs: Vec::new(),
            should_quit: false,
            event_rx,
            command_tx,
            tracker,
            tracker_config,
            repo_path,
            config_dir,
            project_name,
            agents_state: AgentsState::new(),
            stories_state: StoriesState::new(),
            worktrees_state: WorktreesState::new(),
            config_state: ConfigState::new(),
        }
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        // Initial load for config tab
        self.config_state
            .load_config(&self.config_dir, &self.project_name);

        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;

            tokio::select! {
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if event::poll(Duration::from_millis(0))? {
                        if let Event::Key(key) = event::read()? {
                            self.handle_key(key.code, key.modifiers).await;
                        }
                    }
                }
                Some(event) = self.event_rx.recv() => {
                    self.handle_orchestrator_event(event);
                }
            }
        }

        Ok(())
    }

    fn render(&self, frame: &mut ratatui::Frame) {
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
                tabs::agents::render(frame, main_area, &self.runs, &self.agents_state);
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

        widgets::status_bar::render_status_bar(frame, status_area, &self.runs);
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // Global keys
        match code {
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

    async fn handle_agents_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
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
                KeyCode::Char('r') => {
                    if let Some(id) = selected_issue_id {
                        let _ = self
                            .command_tx
                            .send(TuiCommand::RebaseStory { issue_id: id })
                            .await;
                    }
                }
                KeyCode::Char('o') => {
                    if let Some(id) = selected_issue_id {
                        let _ = self
                            .command_tx
                            .send(TuiCommand::CopyWorktreePath { issue_id: id })
                            .await;
                    }
                }
                _ => {}
            },
            AgentFocus::LogPanel => match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if let Some(id) = &selected_issue_id {
                        self.agents_state.scroll_log_down(id);
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if let Some(id) = &selected_issue_id {
                        self.agents_state.scroll_log_up(id);
                    }
                }
                KeyCode::Char('g') => {
                    if let Some(id) = &selected_issue_id {
                        self.agents_state.scroll_to_top(id);
                    }
                }
                KeyCode::Char('G') => {
                    if let Some(id) = &selected_issue_id {
                        self.agents_state.scroll_to_bottom(id);
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

        match code {
            KeyCode::Char('j') | KeyCode::Down => self.stories_state.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.stories_state.move_up(),
            KeyCode::Char('/') => self.stories_state.activate_filter(),
            KeyCode::Char('s') => self.stories_state.toggle_sort(),
            KeyCode::Char('S') => self.stories_state.toggle_sort_direction(),
            KeyCode::Char('r') => self.fetch_stories().await,
            KeyCode::Enter => {
                if let Some(issue) = self.stories_state.selected_issue().cloned() {
                    let _ = self
                        .command_tx
                        .send(TuiCommand::StartStory { issue })
                        .await;
                }
            }
            _ => {}
        }
    }

    async fn handle_worktrees_key(&mut self, code: KeyCode, _modifiers: KeyModifiers) {
        if self.worktrees_state.confirm_delete {
            match code {
                KeyCode::Char('y') => {
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
                if let Some(wt) = self.worktrees_state.selected_worktree() {
                    if let Some(ref branch) = wt.branch {
                        for run in &self.runs {
                            if run.branch.as_deref() == Some(branch) {
                                let _ = self
                                    .command_tx
                                    .send(TuiCommand::RebaseStory {
                                        issue_id: run.issue_id.clone(),
                                    })
                                    .await;
                                break;
                            }
                        }
                    }
                }
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
            status: Some(self.tracker_config.ready_filter.clone()),
        };
        match self.tracker.list_ready(&filters).await {
            Ok(issues) => {
                self.stories_state.issues = issues;
                self.stories_state.loading = false;
            }
            Err(e) => {
                tracing::warn!("Failed to fetch stories: {e}");
                self.stories_state.loading = false;
            }
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
                let line = match &event {
                    AgentEvent::TextDelta(text) => text.clone(),
                    AgentEvent::ToolUse {
                        tool,
                        input_preview,
                    } => {
                        format!("[tool] {tool}: {input_preview}")
                    }
                    AgentEvent::ToolResult { tool, success } => {
                        format!(
                            "[result] {tool}: {}",
                            if *success { "ok" } else { "fail" }
                        )
                    }
                    AgentEvent::Error(msg) => format!("[ERROR] {msg}"),
                    AgentEvent::Complete { cost_usd } => {
                        format!("[complete] cost: ${cost_usd:.2}")
                    }
                    AgentEvent::CostUpdate(cost) => {
                        format!("[cost] ${cost:.2}")
                    }
                };
                self.agents_state.append_log(&issue_id, line);
            }
            OrchestratorEvent::PhaseTransition {
                issue_id,
                from,
                to,
            } => {
                self.agents_state
                    .append_log(&issue_id, format!("--- Phase: {from} -> {to} ---"));
            }
            OrchestratorEvent::StoriesLoaded { issues } => {
                self.stories_state.issues = issues;
                self.stories_state.loading = false;
            }
            OrchestratorEvent::Error { message, .. } => {
                tracing::error!("orchestrator error: {message}");
            }
        }
    }
}
