use crate::config::resolve::load_project_config;
use crate::error::Result;
use crate::state::persistence;

pub fn run_status(repo_path: &str) -> Result<()> {
    let config_dir = crate::app::dirs_config_dir()?;
    let project = load_project_config(&config_dir, repo_path)?;
    let runs_dir = config_dir
        .join("projects")
        .join(&project.name)
        .join("runs");
    let runs = persistence::load_all_runs(&runs_dir)?;

    if runs.is_empty() {
        println!("No active runs for project '{}'.", project.name);
        return Ok(());
    }

    println!(
        "{:<12} {:<30} {:<18} {:<12} {:<8}",
        "Issue", "Title", "Phase", "Status", "Cost"
    );
    println!("{}", "-".repeat(80));

    for run in &runs {
        let title = if run.issue_title.len() > 28 {
            format!("{}...", &run.issue_title[..25])
        } else {
            run.issue_title.clone()
        };
        println!(
            "{:<12} {:<30} {:<18} {:<12} ${:.2}",
            run.issue_id,
            title,
            run.phase.config_key(),
            format!("{:?}", run.status),
            run.cost_usd
        );
    }

    let total: f64 = runs.iter().map(|r| r.cost_usd).sum();
    println!("\n{} runs, ${:.2} total cost", runs.len(), total);
    Ok(())
}
