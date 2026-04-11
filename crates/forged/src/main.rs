mod lock_manager;
mod nats_manager;
mod orchestration;
mod supervisor;
mod system_prompt;

use anyhow::Result;
use clap::Parser;
use forge_core::config::ForgeConfig;
use forge_core::state_file::DaemonState;
use std::path::PathBuf;
use tracing::info;

#[derive(Parser)]
#[command(name = "forged", about = "Forge daemon — orchestration engine")]
struct Cli {
    /// Path to config file.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Working directory for the session.
    #[arg(short, long, default_value = ".")]
    workdir: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = match &cli.config {
        Some(path) => ForgeConfig::load_from(path)?,
        None => ForgeConfig::load()?,
    };

    let workdir = std::fs::canonicalize(&cli.workdir)?;

    // Ensure data directory exists
    let data_dir = ForgeConfig::data_dir();
    std::fs::create_dir_all(&data_dir)?;

    // Set up logging to file
    let log_path = data_dir.join("forged.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "forged=info,forge_core=info".into()),
        )
        .with_target(false)
        .with_ansi(false)
        .with_writer(log_file)
        .init();

    info!("Starting forged in {}", workdir.display());
    info!("Log file: {}", log_path.display());

    // Initialize database
    let db_path = data_dir.join("state.db");
    let db = forge_core::db::init(&db_path)?;
    info!("Database initialized at {}", db_path.display());

    // Start embedded NATS
    let nats_url = nats_manager::start_embedded_nats(&config.nats).await?;
    info!("NATS server started at {}", nats_url);

    // Write state file so forge TUI can find us
    let state = DaemonState {
        pid: std::process::id(),
        nats_url: nats_url.clone(),
        workdir: workdir.to_string_lossy().to_string(),
    };
    state.write()?;
    info!("State file written");

    // Inject forge system prompt into working directory
    system_prompt::inject_system_prompt(&workdir)?;
    info!("System prompt injected into {}", workdir.display());

    // Connect to NATS
    let nats_client = async_nats::connect(&nats_url).await?;
    info!("Connected to NATS");

    // Start lock manager
    let lock_handle = lock_manager::start(db, nats_client.clone(), &config.locks);
    info!("Lock manager started");

    // Set up Ctrl+C / SIGTERM handler
    let shutdown_nats = nats_client.clone();
    tokio::spawn(async move {
        let ctrl_c = tokio::signal::ctrl_c();
        let mut sigterm =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to register SIGTERM handler");

        tokio::select! {
            _ = ctrl_c => info!("Received SIGINT"),
            _ = sigterm.recv() => info!("Received SIGTERM"),
        }

        info!("Initiating graceful shutdown...");

        // Publish shutdown command so orchestration loop exits
        let cmd = forge_core::protocol::CoordinatorCommand::Shutdown;
        let payload = serde_json::to_vec(&cmd).unwrap();
        let _ = shutdown_nats
            .publish(
                forge_core::subjects::CoordinatorSubjects::command(),
                payload.into(),
            )
            .await;
    });

    // Start orchestration engine (spawns coordinator automatically)
    orchestration::run(nats_client.clone(), config, workdir.clone(), lock_handle).await?;

    // Cleanup
    info!("Shutting down...");
    system_prompt::cleanup_system_prompt(&workdir);
    DaemonState::remove();
    info!("Cleanup complete. Goodbye.");

    Ok(())
}
