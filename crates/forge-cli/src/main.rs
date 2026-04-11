mod app;
mod file_tree;
mod input;
mod terminal;
mod tiling;
mod ui;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use forge_core::config::ForgeConfig;
use forge_core::state_file::DaemonState;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "forge", about = "Forge — terminal IDE for agentic coding")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to config file.
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start a new Forge session (starts daemon if needed, then attaches TUI).
    Start {
        /// Working directory.
        #[arg(default_value = ".")]
        workdir: PathBuf,
    },
    /// Attach to an existing Forge session.
    Attach,
    /// Stop the Forge daemon.
    Stop,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = match &cli.config {
        Some(path) => ForgeConfig::load_from(path)?,
        None => ForgeConfig::load()?,
    };

    match cli.command.unwrap_or(Commands::Start {
        workdir: PathBuf::from("."),
    }) {
        Commands::Start { workdir } => {
            let workdir = std::fs::canonicalize(&workdir)?;
            app::run(config, workdir).await?;
        }
        Commands::Attach => {
            let state = DaemonState::read()?
                .context("No running Forge daemon found")?;
            if !state.is_alive() {
                DaemonState::remove();
                anyhow::bail!("Daemon (PID {}) is no longer running", state.pid);
            }
            let workdir = PathBuf::from(&state.workdir);
            app::run(config, workdir).await?;
        }
        Commands::Stop => {
            let state = DaemonState::read()?
                .context("No running Forge daemon found")?;
            if !state.is_alive() {
                DaemonState::remove();
                println!("Daemon was not running (stale state file removed).");
                return Ok(());
            }

            // Connect to NATS and send shutdown command
            let nats = async_nats::connect(&state.nats_url)
                .await
                .context("Failed to connect to daemon's NATS")?;

            let cmd = forge_core::protocol::CoordinatorCommand::Shutdown;
            let payload = serde_json::to_vec(&cmd)?;
            nats.publish(
                forge_core::subjects::CoordinatorSubjects::command(),
                payload.into(),
            )
            .await?;

            nats.flush().await?;

            // Wait briefly for daemon to exit
            for _ in 0..30 {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if !state.is_alive() {
                    println!("Forge daemon stopped.");
                    return Ok(());
                }
            }

            // Force kill if still alive
            println!("Daemon didn't stop gracefully, sending SIGTERM...");
            unsafe {
                libc::kill(state.pid as i32, libc::SIGTERM);
            }
            println!("Done.");
        }
    }

    Ok(())
}
