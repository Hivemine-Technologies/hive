pub mod tabs;
pub mod widgets;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::domain::{OrchestratorEvent, StoryRun, TuiCommand};

use self::tabs::Tab;

pub struct Tui {
    active_tab: Tab,
    runs: Vec<StoryRun>,
    selected_agent: usize,
    should_quit: bool,
    event_rx: mpsc::Receiver<OrchestratorEvent>,
    command_tx: mpsc::Sender<TuiCommand>,
}

impl Tui {
    pub fn new(
        event_rx: mpsc::Receiver<OrchestratorEvent>,
        command_tx: mpsc::Sender<TuiCommand>,
    ) -> Self {
        Self {
            active_tab: Tab::Agents,
            runs: Vec::new(),
            selected_agent: 0,
            should_quit: false,
            event_rx,
            command_tx,
        }
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
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
                tabs::agents::render(frame, main_area, &self.runs, self.selected_agent);
            }
            Tab::Stories => tabs::stories::render(frame, main_area),
            Tab::Worktrees => tabs::worktrees::render(frame, main_area),
            Tab::Config => tabs::config_tab::render(frame, main_area),
        }

        widgets::status_bar::render_status_bar(frame, status_area, &self.runs);
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        match code {
            KeyCode::Char('q') => {
                let _ = self.command_tx.send(TuiCommand::Quit).await;
                self.should_quit = true;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.command_tx.send(TuiCommand::Quit).await;
                self.should_quit = true;
            }
            KeyCode::Tab => self.active_tab = self.active_tab.next(),
            KeyCode::BackTab => self.active_tab = self.active_tab.prev(),
            KeyCode::Char('1') => self.active_tab = Tab::Agents,
            KeyCode::Char('2') => self.active_tab = Tab::Stories,
            KeyCode::Char('3') => self.active_tab = Tab::Worktrees,
            KeyCode::Char('4') => self.active_tab = Tab::Config,
            KeyCode::Char('j') | KeyCode::Down => {
                if self.active_tab == Tab::Agents && !self.runs.is_empty() {
                    self.selected_agent = (self.selected_agent + 1).min(self.runs.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.active_tab == Tab::Agents && self.selected_agent > 0 {
                    self.selected_agent -= 1;
                }
            }
            _ => {}
        }
    }

    fn handle_orchestrator_event(&mut self, event: OrchestratorEvent) {
        match event {
            OrchestratorEvent::StoryUpdated(run) => {
                if let Some(existing) = self.runs.iter_mut().find(|r| r.issue_id == run.issue_id) {
                    *existing = run;
                } else {
                    self.runs.push(run);
                }
            }
            OrchestratorEvent::Error { message, .. } => {
                tracing::error!("orchestrator error: {message}");
            }
            _ => {}
        }
    }
}
