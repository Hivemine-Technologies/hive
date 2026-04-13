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

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Tab::Agents,
            1 => Tab::Stories,
            2 => Tab::Worktrees,
            3 => Tab::Config,
            _ => Tab::Agents,
        }
    }

    pub fn next(&self) -> Self {
        Self::from_index((self.index() + 1) % Self::all().len())
    }

    pub fn prev(&self) -> Self {
        let len = Self::all().len();
        Self::from_index((self.index() + len - 1) % len)
    }
}
