pub mod agents;
pub mod config_tab;
pub mod stories;
pub mod worktrees;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Agents,
    Stories,
    Worktrees,
    Config,
}

impl Tab {
    pub fn all() -> &'static [Tab] {
        &[Tab::Agents, Tab::Stories, Tab::Worktrees, Tab::Config]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Agents => "Agents",
            Tab::Stories => "Stories",
            Tab::Worktrees => "Worktrees",
            Tab::Config => "Config",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::Agents => 0,
            Tab::Stories => 1,
            Tab::Worktrees => 2,
            Tab::Config => 3,
        }
    }

}
