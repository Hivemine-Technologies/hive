mod app;
mod cli;
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
use tracing_subscriber::{fmt, layer::SubscriberExt, layer::Layer, util::SubscriberInitExt, EnvFilter};

#[derive(Parser)]
#[command(
    name = "hive",
    version,
    about = "Agent orchestration TUI for story-to-PR automation",
    long_about = "Hive orchestrates autonomous coding agents through a story-to-PR pipeline.\n\n\
        Run `hive` with no subcommand to launch the TUI dashboard.\n\
        Run `hive init` to set up a new project.\n\n\
        Required env vars: GITHUB_TOKEN (or GH_TOKEN), LINEAR_API_KEY (or JIRA_API_TOKEN)"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Set up a new project (interactive wizard)
    Init,
    /// Reconfigure the current project (edit existing settings)
    Configure,
    /// Print a summary of all active story runs
    Status,
}

#[tokio::main]
async fn main() {
    // Persistent file logging — daily rotation to ~/.config/hive/logs/
    let log_dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config").join("hive").join("logs"))
        .unwrap_or_else(|_| std::path::PathBuf::from("logs"));
    if let Err(e) = std::fs::create_dir_all(&log_dir) {
        eprintln!("warning: could not create log directory {}: {e}", log_dir.display());
    }

    let file_appender = tracing_appender::rolling::daily(&log_dir, "hive.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    let cli = Cli::parse();
    let is_tui = cli.command.is_none();

    if is_tui {
        // TUI mode: file logging only — stderr would corrupt the terminal
        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_filter(EnvFilter::new("hive=debug")),
            )
            .init();
    } else {
        // CLI mode: stderr + file logging
        tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_filter(EnvFilter::new("hive=info")),
            )
            .with(
                fmt::layer()
                    .with_writer(non_blocking)
                    .with_ansi(false)
                    .with_filter(EnvFilter::new("hive=debug")),
            )
            .init();
    }

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
            if let Err(e) = cli::init::run_init() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Configure) => {
            if let Err(e) = cli::configure::run_configure() {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Status) => {
            let cwd = std::env::current_dir()
                .expect("cannot determine current directory")
                .to_string_lossy()
                .to_string();
            if let Err(e) = cli::status::run_status(&cwd) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
    }
}
