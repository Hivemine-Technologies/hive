mod app;
mod config;
mod domain;
mod error;
mod git;
mod notifiers;
mod orchestrator;
mod runners;
mod state;
mod trackers;
mod tui;

use clap::Parser;

#[derive(Parser)]
#[command(name = "hive", version, about = "Agent orchestration TUI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Initialize a new project
    Init,
    /// Re-run the setup wizard
    Configure,
    /// Print status of all active runs
    Status,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("hive=info")
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            let cwd = std::env::current_dir()
                .expect("cannot determine current directory")
                .to_string_lossy()
                .to_string();
            if let Err(e) = app::run(&cwd).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Init) => {
            println!("hive init — not yet implemented");
        }
        Some(Commands::Configure) => {
            println!("hive configure — not yet implemented");
        }
        Some(Commands::Status) => {
            println!("hive status — not yet implemented");
        }
    }
}
