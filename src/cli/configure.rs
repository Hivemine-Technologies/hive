use crate::config::resolve::load_project_config;
use crate::error::Result;

pub fn run_configure() -> Result<()> {
    let config_dir = crate::app::dirs_config_dir()?;
    let cwd = std::env::current_dir()?.to_string_lossy().to_string();
    let existing = load_project_config(&config_dir, &cwd)?;
    super::wizard::run_wizard(Some(existing))
}
