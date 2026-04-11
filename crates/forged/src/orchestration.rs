//! Orchestration engine — manages agent lifecycle and coordinates work.

use crate::lock_manager::LockManagerHandle;
use crate::supervisor::local::LocalSupervisor;
use crate::supervisor::Supervisor;
use anyhow::Result;
use futures_util::StreamExt;
use forge_core::config::{AgentType, ForgeConfig};
use forge_core::protocol::{AgentStateEvent, CoordinatorCommand, CoordinatorNotification, FsAction, FsEvent, SessionSnapshot};
use forge_core::subjects::{AgentSubjects, CoordinatorInbox, CoordinatorSubjects, SessionSubjects};
use forge_core::types::{AgentColor, AgentHost, AgentId, AgentInfo, AgentState};
use forge_core::db;
use rusqlite::Connection;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// Per-agent output ring buffer (last N lines).
const OUTPUT_BUFFER_LINES: usize = 200;

struct AgentEntry {
    info: AgentInfo,
    supervisor: Box<dyn Supervisor>,
    /// Ring buffer of recent output lines for read_output.
    output_buffer: VecDeque<String>,
}

/// Run the orchestration engine until shutdown.
pub async fn run(
    nats: async_nats::Client,
    config: ForgeConfig,
    workdir: PathBuf,
    _lock_handle: LockManagerHandle,
    db: Arc<std::sync::Mutex<Connection>>,
) -> Result<()> {
    let agents: Arc<Mutex<HashMap<String, AgentEntry>>> = Arc::new(Mutex::new(HashMap::new()));

    // Auto-spawn coordinator agent
    let coord_id = spawn_agent_full(
        &nats,
        &config,
        &agents,
        "coordinator".to_string(),
        None,   // use default agent type
        None,
        None,
        AgentHost::Local,
        workdir.to_string_lossy().to_string(),
        None,   // no initial task for coordinator
        true,   // is_coordinator
    )
    .await?;
    info!("Coordinator agent spawned: {}", coord_id);

    // Start filesystem watcher — watches workdir and publishes FsEvent to NATS.
    // The coordinator's fs subject (forge.agent.{coord_id}.fs) carries system-level events;
    // individual agents report their own changes via forge_notify_done.
    start_fs_watcher(nats.clone(), workdir.clone(), coord_id.clone());

    // Subscribe to coordinator commands
    let mut cmd_sub = nats.subscribe(CoordinatorSubjects::command()).await?;
    info!("Orchestration engine listening on {}", CoordinatorSubjects::command());

    // Subscribe to agent state changes
    let mut state_sub = nats.subscribe(AgentSubjects::all_states()).await?;

    // Subscribe to session handshake requests
    let mut session_sub = nats.subscribe(SessionSubjects::state()).await?;

    // Subscribe to all agent stdout for output buffering
    let mut stdout_sub = nats.subscribe(AgentSubjects::all_stdout()).await?;

    // Subscribe to coordinator notifications (for PTY injection)
    let mut notif_sub = nats.subscribe(CoordinatorInbox::notifications()).await?;

    let agents_for_state = agents.clone();
    let agents_for_session = agents.clone();
    let agents_for_stdout = agents.clone();
    let agents_for_notif = agents.clone();
    let nats_for_session = nats.clone();
    let nats_for_notif = nats.clone();
    let coord_id_for_notif = coord_id.clone();
    let db_for_stdout: Arc<std::sync::Mutex<Connection>> = Arc::clone(&db);

    // Handle state changes in background
    tokio::spawn(async move {
        while let Some(msg) = state_sub.next().await {
            if let Ok(event) = serde_json::from_slice::<AgentStateEvent>(&msg.payload) {
                let mut agents = agents_for_state.lock().await;
                if let Some(entry) = agents.get_mut(event.agent_id.as_str()) {
                    entry.info.state = event.state.clone();
                    info!(
                        "Agent {} state -> {}{}",
                        event.agent_id,
                        event.state,
                        event.message.as_ref().map(|m| format!(": {m}")).unwrap_or_default()
                    );
                }
            }
        }
    });

    // Handle session handshake requests in background
    tokio::spawn(async move {
        while let Some(msg) = session_sub.next().await {
            if let Some(reply) = msg.reply {
                let agents = agents_for_session.lock().await;
                let agent_list: Vec<AgentInfo> = agents.values().map(|e| e.info.clone()).collect();
                let snapshot = SessionSnapshot {
                    session_id: "default".to_string(),
                    agents: agent_list,
                    locks: Vec::new(),
                    nats_url: String::new(),
                };
                let payload = serde_json::to_vec(&snapshot).unwrap();
                let _ = nats_for_session.publish(reply, payload.into()).await;
            }
        }
    });

    // Buffer agent stdout output in background + detect state changes + track cwd
    let nats_for_state_detect = nats.clone();
    tokio::spawn(async move {
        while let Some(msg) = stdout_sub.next().await {
            if let Some(agent_id) = extract_agent_id(&msg.subject) {
                let mut agents = agents_for_stdout.lock().await;
                if let Some(entry) = agents.get_mut(&agent_id) {
                    let text = String::from_utf8_lossy(&msg.payload);

                    // Check for OSC 7 (cwd tracking): \x1b]7;file://hostname/path\x07
                    if let Some(cwd) = extract_osc7_cwd(&text) {
                        if entry.info.working_dir != cwd {
                            entry.info.working_dir = cwd.clone();
                            // Persist to SQLite so attach sessions see the latest cwd.
                            let conn = db_for_stdout.lock().unwrap();
                            let _ = db::update_agent_working_dir(&conn, &entry.info.id, &cwd);
                        }
                    }

                    // Buffer lines and detect state changes
                    for line in text.lines() {
                        entry.output_buffer.push_back(line.to_string());
                        if entry.output_buffer.len() > OUTPUT_BUFFER_LINES {
                            entry.output_buffer.pop_front();
                        }

                        // State detection via output heuristics
                        if let Some(new_state) = crate::state_detector::detect_state(line) {
                            let current = &entry.info.state;
                            // Only allow sensible transitions
                            let should_transition = match (&current, &new_state) {
                                // Don't resurrect done/error agents
                                (AgentState::Done, _) => false,
                                (AgentState::Error, _) => false,
                                // Allow all other transitions
                                _ => true,
                            };

                            if should_transition && *current != new_state {
                                entry.info.state = new_state.clone();
                                let event = AgentStateEvent {
                                    agent_id: AgentId(agent_id.clone()),
                                    state: new_state,
                                    message: Some(format!("Detected from output: {}", line.chars().take(80).collect::<String>())),
                                };
                                let payload = serde_json::to_vec(&event).unwrap();
                                let _ = nats_for_state_detect
                                    .publish(AgentSubjects::state(&AgentId(agent_id.clone())), payload.into())
                                    .await;
                            }
                        }
                    }
                }
            }
        }
    });

    // Deliver coordinator notifications by typing into coordinator PTY
    tokio::spawn(async move {
        while let Some(msg) = notif_sub.next().await {
            if let Ok(notif) = serde_json::from_slice::<CoordinatorNotification>(&msg.payload) {
                let agents = agents_for_notif.lock().await;
                if let Some(coord_entry) = agents.get(coord_id_for_notif.as_str()) {
                    let formatted = format!(
                        "\n[FORGE] Agent \"{}\" ({}): {}\n",
                        notif.from_label,
                        match notif.notification_type {
                            forge_core::protocol::NotificationType::Done => "DONE",
                            forge_core::protocol::NotificationType::Status => "STATUS",
                            forge_core::protocol::NotificationType::Message => "MESSAGE",
                            forge_core::protocol::NotificationType::InputRequest => "INPUT NEEDED",
                        },
                        notif.content
                    );
                    // Inject into coordinator's PTY stdin via NATS
                    let _ = nats_for_notif
                        .publish(
                            AgentSubjects::stdin(&coord_id_for_notif),
                            formatted.into(),
                        )
                        .await;
                }
            }
        }
    });

    // Process coordinator commands
    while let Some(msg) = cmd_sub.next().await {
        let command: CoordinatorCommand = match serde_json::from_slice(&msg.payload) {
            Ok(c) => c,
            Err(e) => {
                warn!("Invalid coordinator command: {e}");
                continue;
            }
        };

        match command {
            CoordinatorCommand::Shutdown => {
                info!("Shutdown command received — killing all agents");
                let mut agents = agents.lock().await;
                for (id, entry) in agents.iter_mut() {
                    info!("Killing agent {}", id);
                    let _ = entry.supervisor.kill();
                }
                agents.clear();
                break;
            }

            CoordinatorCommand::SpawnAgent {
                label,
                agent_type,
                command,
                args,
                host,
                working_dir,
                task,
            } => {
                match spawn_agent_full(
                    &nats, &config, &agents, label, agent_type, command, args,
                    host, working_dir, task, false,
                )
                .await
                {
                    Ok(id) => info!("Spawned agent {}", id),
                    Err(e) => {
                        error!("Failed to spawn agent: {e}");
                        // If this was a request (has reply), send error back
                        if let Some(reply) = msg.reply {
                            let err = serde_json::json!({ "error": format!("{e}") });
                            let _ = nats.publish(reply, serde_json::to_vec(&err).unwrap().into()).await;
                        }
                    }
                }
            }

            CoordinatorCommand::KillAgent { agent_id } => {
                let mut agents = agents.lock().await;
                if let Some(entry) = agents.get_mut(agent_id.as_str()) {
                    if let Err(e) = entry.supervisor.kill() {
                        error!("Failed to kill agent {}: {e}", agent_id);
                    } else {
                        entry.info.state = AgentState::Done;
                        info!("Killed agent {}", agent_id);
                        let event = AgentStateEvent {
                            agent_id: agent_id.clone(),
                            state: AgentState::Done,
                            message: Some("Killed by user".to_string()),
                        };
                        let payload = serde_json::to_vec(&event).unwrap();
                        let _ = nats.publish(AgentSubjects::state(&agent_id), payload.into()).await;
                    }
                }
            }

            CoordinatorCommand::SendMessage { agent_id, message } => {
                // Type the message into the agent's PTY stdin
                let _ = nats
                    .publish(AgentSubjects::stdin(&agent_id), message.into())
                    .await;
            }

            CoordinatorCommand::ReadOutput {
                agent_id,
                last_n_lines,
            } => {
                let agents = agents.lock().await;
                let lines: Vec<String> = if let Some(entry) = agents.get(agent_id.as_str()) {
                    let buf = &entry.output_buffer;
                    let skip = buf.len().saturating_sub(last_n_lines);
                    buf.iter().skip(skip).cloned().collect()
                } else {
                    vec![]
                };

                let result = serde_json::json!({ "lines": lines });
                if let Some(reply) = msg.reply {
                    let _ = nats
                        .publish(reply, serde_json::to_vec(&result).unwrap().into())
                        .await;
                }
            }

            CoordinatorCommand::ResizeAgent { agent_id, cols, rows } => {
                let agents = agents.lock().await;
                if let Some(entry) = agents.get(agent_id.as_str()) {
                    if let Err(e) = entry.supervisor.resize(rows, cols) {
                        warn!("Failed to resize agent {}: {e}", agent_id);
                    }
                }
            }

            CoordinatorCommand::ForceReleaseLock { path } => {
                let request = forge_core::protocol::LockRequest::ForceRelease { path };
                let payload = serde_json::to_vec(&request).unwrap();
                let _ = nats
                    .publish(forge_core::subjects::FileLockSubjects::request(), payload.into())
                    .await;
            }
        }
    }

    info!("Orchestration engine stopped");
    Ok(())
}

/// Spawn a new agent with full options.
#[allow(clippy::too_many_arguments)]
async fn spawn_agent_full(
    nats: &async_nats::Client,
    config: &ForgeConfig,
    agents: &Arc<Mutex<HashMap<String, AgentEntry>>>,
    label: String,
    agent_type_name: Option<String>,
    command_override: Option<String>,
    args_override: Option<Vec<String>>,
    host: AgentHost,
    working_dir: String,
    task: Option<String>,
    is_coordinator: bool,
) -> Result<AgentId> {
    let agent_id = AgentId::new();

    // Resolve agent type from registry
    let type_name = agent_type_name
        .clone()
        .unwrap_or_else(|| config.agent.command.clone());
    let resolved: AgentType = config.agents.resolve(&type_name);

    let cmd = command_override.unwrap_or(resolved.command.clone());
    let mut cmd_args = args_override.unwrap_or_else(|| {
        let mut a = resolved.args.clone();
        a.extend(config.agent.args.clone());
        a
    });

    let mcp_capable = resolved.mcp;

    // If MCP capable, generate MCP config file and inject it into args
    let mcp_config_path = if mcp_capable {
        let nats_url = nats.server_info().client_ip.clone();
        // Use the NATS URL from the state file
        let state = forge_core::state_file::DaemonState::read()?.unwrap_or_else(|| {
            forge_core::state_file::DaemonState {
                pid: 0,
                nats_url: "nats://127.0.0.1:4222".to_string(),
                workdir: String::new(),
            }
        });

        let mcp_config = generate_mcp_config(
            &agent_id,
            if is_coordinator { "coordinator" } else { "subagent" },
            &state.nats_url,
        )?;

        // Write MCP config to a temp file
        let config_path = forge_core::config::ForgeConfig::data_dir()
            .join(format!("mcp-{}.json", agent_id));
        std::fs::write(&config_path, &mcp_config)?;

        if let Some(ref flag) = resolved.mcp_config_flag {
            cmd_args.push(flag.clone());
            cmd_args.push(config_path.to_string_lossy().to_string());
        }

        Some(config_path)
    } else {
        None
    };

    let agent_count = agents.lock().await.len();
    let color = AgentColor::from_index(agent_count);

    let info = AgentInfo {
        id: agent_id.clone(),
        label: label.clone(),
        command: cmd.clone(),
        args: cmd_args.clone(),
        host: host.clone(),
        working_dir: working_dir.clone(),
        state: AgentState::Starting,
        color,
        mcp_capable,
        agent_type: type_name,
    };

    match &host {
        AgentHost::Local => {
            let mut supervisor = LocalSupervisor::new(
                agent_id.clone(),
                cmd,
                cmd_args,
                working_dir,
            );
            supervisor.start_with_nats(nats.clone())?;

            let mut agents = agents.lock().await;
            agents.insert(
                agent_id.as_str().to_string(),
                AgentEntry {
                    info,
                    supervisor: Box::new(supervisor),
                    output_buffer: VecDeque::new(),
                },
            );

            let event = AgentStateEvent {
                agent_id: agent_id.clone(),
                state: AgentState::Running,
                message: None,
            };
            let payload = serde_json::to_vec(&event).unwrap();
            let _ = nats.publish(AgentSubjects::state(&agent_id), payload.into()).await;

            // Inject initial task if provided (wait for agent to be ready)
            if let Some(task_text) = task {
                let nats_clone = nats.clone();
                let agent_id_clone = agent_id.clone();
                tokio::spawn(async move {
                    // Wait for the agent CLI to initialize
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    let _ = nats_clone
                        .publish(
                            AgentSubjects::stdin(&agent_id_clone),
                            format!("{task_text}\n").into(),
                        )
                        .await;
                });
            }
        }
        AgentHost::Remote { name } => {
            anyhow::bail!("Remote agents not yet implemented (host: {name})");
        }
    }

    Ok(agent_id)
}

/// Generate MCP config JSON for an agent.
fn generate_mcp_config(agent_id: &AgentId, role: &str, nats_url: &str) -> Result<String> {
    let forge_mcp_path = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("forge-mcp");

    let config = serde_json::json!({
        "mcpServers": {
            "forge": {
                "command": forge_mcp_path.to_string_lossy(),
                "env": {
                    "FORGE_NATS_URL": nats_url,
                    "FORGE_AGENT_ID": agent_id.as_str(),
                    "FORGE_AGENT_ROLE": role
                }
            }
        }
    });

    Ok(serde_json::to_string_pretty(&config)?)
}

/// Extract agent ID from subject like "forge.agent.{id}.stdout"
fn extract_agent_id(subject: &str) -> Option<String> {
    let parts: Vec<&str> = subject.split('.').collect();
    if parts.len() >= 4 && parts[0] == "forge" && parts[1] == "agent" {
        Some(parts[2].to_string())
    } else {
        None
    }
}

/// Extract working directory from OSC 7 escape sequence.
///
/// Modern shells (bash, zsh, fish) emit: `\x1b]7;file://hostname/path/to/dir\x07`
/// to advertise the current working directory. This function parses the path out.
fn extract_osc7_cwd(text: &str) -> Option<String> {
    // Look for OSC 7 pattern: \x1b]7;file://... terminated by \x07 or \x1b\\
    let osc_start = text.find("\x1b]7;")?;
    let after_osc = &text[osc_start + 4..]; // skip \x1b]7;

    // Find terminator: BEL (\x07) or ST (\x1b\\)
    let end = after_osc.find('\x07')
        .or_else(|| after_osc.find("\x1b\\"))?;

    let uri = &after_osc[..end];

    // Parse file:// URI
    if let Some(path_start) = uri.strip_prefix("file://") {
        // Skip hostname (everything up to the next /)
        if let Some(slash_pos) = path_start.find('/') {
            let path = &path_start[slash_pos..];
            // URL-decode percent-encoded characters
            let decoded = percent_decode(path);
            if !decoded.is_empty() {
                return Some(decoded);
            }
        }
    }

    None
}

/// Simple percent-decoding for file paths.
fn percent_decode(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }
    result
}

// ─── Filesystem Watcher ─────────────────────────────────────────────

/// Start a background notify watcher on `workdir`.
/// File system events are published as `FsEvent` to `forge.agent.{coord_id}.fs`.
fn start_fs_watcher(nats: async_nats::Client, workdir: PathBuf, coord_id: AgentId) {
    use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
    use notify::event::{CreateKind, ModifyKind, RemoveKind};

    let (sync_tx, sync_rx) = std::sync::mpsc::channel::<notify::Event>();
    let mut watcher = match RecommendedWatcher::new(
        move |res: notify::Result<notify::Event>| {
            if let Ok(ev) = res {
                let _ = sync_tx.send(ev);
            }
        },
        notify::Config::default(),
    ) {
        Ok(w) => w,
        Err(e) => {
            warn!("Failed to create fs watcher: {e}");
            return;
        }
    };

    if let Err(e) = watcher.watch(&workdir, RecursiveMode::Recursive) {
        warn!("Failed to watch {}: {e}", workdir.display());
        return;
    }

    info!("Filesystem watcher started on {}", workdir.display());

    // Bridge sync notify channel → async NATS publishes via a blocking thread.
    let (async_tx, mut async_rx) = tokio::sync::mpsc::unbounded_channel::<notify::Event>();
    std::thread::spawn(move || {
        let _watcher = watcher; // keep watcher alive for this thread's lifetime
        for event in sync_rx {
            if async_tx.send(event).is_err() {
                break;
            }
        }
    });

    tokio::spawn(async move {
        let subject = AgentSubjects::fs(&coord_id);
        while let Some(event) = async_rx.recv().await {
            // Map notify EventKind to FsAction; skip events we don't care about.
            let action = match &event.kind {
                EventKind::Create(CreateKind::File) | EventKind::Create(_) => FsAction::Create,
                EventKind::Modify(ModifyKind::Name(_)) => FsAction::Modify,
                EventKind::Modify(_) => FsAction::Modify,
                EventKind::Remove(RemoveKind::File) | EventKind::Remove(_) => FsAction::Delete,
                EventKind::Access(_) | EventKind::Other | EventKind::Any => continue,
            };

            for path in &event.paths {
                let path_str = path.to_string_lossy().to_string();

                // Skip hidden dirs and build artefact dirs
                if should_ignore_path(&path_str) {
                    continue;
                }

                let fs_event = FsEvent {
                    agent_id: coord_id.clone(),
                    action: action.clone(),
                    path: path_str,
                };
                if let Ok(payload) = serde_json::to_vec(&fs_event) {
                    let _ = nats.publish(subject.clone(), payload.into()).await;
                }
            }
        }
    });
}

/// Return true if a path should be excluded from fs event broadcasting.
fn should_ignore_path(path: &str) -> bool {
    path.split('/').any(|seg| {
        matches!(
            seg,
            ".git" | "target" | "node_modules" | ".forge-instructions.md"
        )
    })
}
