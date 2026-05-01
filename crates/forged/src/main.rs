mod lock_manager;
mod nats_manager;
mod orchestration;
mod state_detector;
mod supervisor;
mod system_prompt;

use anyhow::Result;
use clap::Parser;
use forge_core::config::ForgeConfig;
use forge_core::state_file::DaemonState;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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

    /// Run as a fully detached daemon (double-fork).
    #[arg(long)]
    daemon: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let config = match &cli.config {
        Some(path) => ForgeConfig::load_from(path)?,
        None => ForgeConfig::load()?,
    };

    let workdir = std::fs::canonicalize(&cli.workdir)?;

    // Double-fork to fully daemonize if --daemon was passed
    if cli.daemon {
        unsafe {
            let pid = libc::fork();
            if pid < 0 {
                anyhow::bail!("First fork failed");
            }
            if pid > 0 {
                // Parent: print child PID and exit
                std::process::exit(0);
            }
            // Child: create new session
            libc::setsid();
            let pid2 = libc::fork();
            if pid2 < 0 {
                anyhow::bail!("Second fork failed");
            }
            if pid2 > 0 {
                // First child exits; grandchild continues as daemon
                std::process::exit(0);
            }
            // Grandchild: redirect stdin/stdout/stderr to /dev/null
            let devnull = libc::open(b"/dev/null\0".as_ptr() as *const _, libc::O_RDWR);
            if devnull >= 0 {
                libc::dup2(devnull, 0);
                libc::dup2(devnull, 1);
                libc::dup2(devnull, 2);
                if devnull > 2 {
                    libc::close(devnull);
                }
            }
        }
    }

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
    let db = Arc::new(Mutex::new(forge_core::db::init(&db_path)?));
    info!("Database initialized at {}", db_path.display());

    // Start embedded NATS
    let nats_url = nats_manager::start_embedded_nats(&config.nats).await?;
    info!("NATS server started at {}", nats_url);

    // Generate a session ID
    let session_id = forge_core::types::SessionId::new();

    // Write state file so forge TUI can find us
    let state = DaemonState {
        pid: std::process::id(),
        nats_url: nats_url.clone(),
        workdir: workdir.to_string_lossy().to_string(),
        session_id: session_id.as_str().to_string(),
    };
    state.write()?;
    info!("State file written (session: {})", session_id);

    // Inject forge system prompt into working directory
    system_prompt::inject_system_prompt(&workdir)?;
    info!("System prompt injected into {}", workdir.display());

    // Connect to NATS
    let nats_client = async_nats::connect(&nats_url).await?;
    info!("Connected to NATS");

    // Start lock manager
    let lock_handle = lock_manager::start(Arc::clone(&db), nats_client.clone(), &config.locks);
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
    orchestration::run(nats_client.clone(), config, workdir.clone(), lock_handle, Arc::clone(&db)).await?;

    // Cleanup
    info!("Shutting down...");
    system_prompt::cleanup_system_prompt(&workdir);
    DaemonState::remove();
    info!("Cleanup complete. Goodbye.");

    Ok(())
}
