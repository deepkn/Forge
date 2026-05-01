//! Main application loop — event handling and render orchestration.

use crate::file_tree::FileTreeState;
use crate::input::{self, InputEvent};
use crate::terminal::TerminalState;
use crate::tiling::TilingLayout;
use crate::ui;
use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use forge_core::config::ForgeConfig;
use forge_core::protocol::{AgentStateEvent, CoordinatorCommand, FsEvent, RemoteSnapshot, SessionSnapshot, UiEvent};
use forge_core::state_file::DaemonState;
use forge_core::subjects::{AgentSubjects, CoordinatorSubjects, SessionSubjects};
use forge_core::types::{AgentId, AgentInfo};
use futures_util::StreamExt;
use ratatui::prelude::*;
use std::collections::HashMap;
use std::io::stdout;
use std::path::PathBuf;
use std::time::Duration;

/// Pane focus target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FocusTarget {
    FileTree,
    Coordinator,
    Agent(AgentId),
}

/// Application state.
pub struct App {
    pub config: ForgeConfig,
    pub workdir: PathBuf,
    pub running: bool,
    pub focus: FocusTarget,
    pub agents: HashMap<String, AgentInfo>,
    pub tiling: TilingLayout,
    pub file_tree_visible: bool,
    pub coordinator_visible: bool,
    pub zoomed: bool,
    pub terminals: HashMap<String, TerminalState>,
    pub coordinator_id: Option<AgentId>,
    pub file_tree: FileTreeState,
    /// True when Esc prefix was pressed, waiting for command key.
    pub command_mode: bool,
    /// Set when layout changes require PTY resize propagation.
    pub needs_resize: bool,
}

impl App {
    pub fn new(config: ForgeConfig, workdir: PathBuf) -> Self {
        let mut file_tree = FileTreeState::new();
        file_tree.scan(&workdir);

        Self {
            config,
            workdir,
            running: true,
            focus: FocusTarget::Coordinator,
            agents: HashMap::new(),
            tiling: TilingLayout::new(),
            file_tree_visible: true,
            coordinator_visible: true,
            zoomed: false,
            terminals: HashMap::new(),
            coordinator_id: None,
            file_tree,
            command_mode: false,
            needs_resize: true, // Force initial resize on first frame
        }
    }

    pub fn coordinator_terminal(&self) -> Option<&TerminalState> {
        self.coordinator_id
            .as_ref()
            .and_then(|id| self.terminals.get(id.as_str()))
    }

    pub fn agent_terminal(&self, id: &AgentId) -> Option<&TerminalState> {
        self.terminals.get(id.as_str())
    }

    /// Register an agent from a state event or session snapshot.
    pub fn register_agent(&mut self, info: AgentInfo) {
        let id_str = info.id.as_str().to_string();

        // First agent registered becomes the coordinator
        if self.coordinator_id.is_none() {
            self.coordinator_id = Some(info.id.clone());
        } else {
            // Non-coordinator agents get tiling panes
            self.tiling.add_pane(info.id.clone());
        }

        self.agents.insert(id_str, info);
        self.needs_resize = true;
    }

    /// Ensure a terminal exists for the given agent, sized to the given dimensions.
    pub fn ensure_terminal(&mut self, id: &str, cols: u16, rows: u16) {
        self.terminals
            .entry(id.to_string())
            .or_insert_with(|| TerminalState::new(cols, rows));
    }

    /// Calculate the correct terminal dimensions for the coordinator pane.
    pub fn coordinator_pane_size(&self, total_cols: u16, total_rows: u16) -> (u16, u16) {
        let cols = self.config.ui.coordinator_width.saturating_sub(2); // borders
        let rows = total_rows.saturating_sub(5); // dock + borders
        (cols, rows)
    }

    /// Calculate the correct terminal dimensions for center (subagent) panes.
    pub fn center_pane_size(&self, total_cols: u16, total_rows: u16) -> (u16, u16) {
        let mut used_cols: u16 = 0;
        if self.file_tree_visible {
            used_cols += self.config.ui.file_tree_width;
        }
        if self.coordinator_visible {
            used_cols += self.config.ui.coordinator_width;
        }
        let center_cols = total_cols.saturating_sub(used_cols).saturating_sub(2); // borders
        let center_rows = total_rows.saturating_sub(5); // dock + borders
        (center_cols, center_rows)
    }

    /// Check if an agent is the coordinator.
    pub fn is_coordinator(&self, id: &str) -> bool {
        self.coordinator_id.as_ref().map(|c| c.as_str()) == Some(id)
    }

    /// Try to switch focus to whichever agent owns the given file path.
    /// Checks active_edits first, then lock holders. No-ops if no agent is found.
    fn focus_agent_for_path(&mut self, path: &str) {
        let agent_id = self
            .file_tree
            .active_edits
            .get(path)
            .cloned()
            .or_else(|| {
                self.file_tree
                    .locks
                    .get(path)
                    .and_then(|v| v.first())
                    .map(|l| l.agent_id.clone())
            });

        if let Some(id) = agent_id {
            if self.coordinator_id.as_ref() == Some(&id) {
                self.focus = FocusTarget::Coordinator;
            } else {
                // Only focus if the agent actually has a tiling pane.
                let has_pane = self.tiling.leaves().iter().any(|l| l.as_ref() == Some(&id));
                if has_pane {
                    self.focus = FocusTarget::Agent(id);
                }
            }
        }
    }

    pub fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::Quit => self.running = false,
            InputEvent::ToggleFileTree => {
                self.file_tree_visible = !self.file_tree_visible;
                self.needs_resize = true;
            }
            InputEvent::ToggleCoordinator => {
                self.coordinator_visible = !self.coordinator_visible;
                self.needs_resize = true;
            }
            InputEvent::FocusLeft => self.move_focus_left(),
            InputEvent::FocusRight => self.move_focus_right(),
            InputEvent::FocusUp => self.tiling.focus_prev(),
            InputEvent::FocusDown => self.tiling.focus_next(),
            InputEvent::ZoomToggle => {
                self.zoomed = !self.zoomed;
                self.needs_resize = true;
            }
            InputEvent::SplitHorizontal => {
                self.tiling.split_horizontal();
                self.needs_resize = true;
            }
            InputEvent::SplitVertical => {
                self.tiling.split_vertical();
                self.needs_resize = true;
            }
            InputEvent::ClosePane => {
                self.tiling.close_focused();
                self.needs_resize = true;
            }
            InputEvent::FocusDockItem(n) => self.focus_dock_item(n),
            InputEvent::ToggleFullscreen => {
                self.file_tree_visible = !self.file_tree_visible;
                self.coordinator_visible = !self.coordinator_visible;
                self.needs_resize = true;
            }
            InputEvent::ResizeLeft => {
                self.tiling.resize_focused(false);
                self.needs_resize = true;
            }
            InputEvent::ResizeRight => {
                self.tiling.resize_focused(true);
                self.needs_resize = true;
            }
            InputEvent::ResizeUp => {
                self.tiling.resize_focused(true);
                self.needs_resize = true;
            }
            InputEvent::ResizeDown => {
                self.tiling.resize_focused(false);
                self.needs_resize = true;
            }
            InputEvent::TreeUp => self.file_tree.select_prev(),
            InputEvent::TreeDown => self.file_tree.select_next(),
            InputEvent::TreeExpand => self.file_tree.expand(),
            InputEvent::TreeCollapse => self.file_tree.collapse(),
            InputEvent::TreeToggle => {
                // If the selected item is a directory, expand/collapse it.
                // If it's a file, switch focus to the agent that holds it.
                let sel_info = self.file_tree.visible.get(self.file_tree.selected)
                    .map(|n| (n.is_dir, n.path.to_string_lossy().to_string()));
                match sel_info {
                    Some((true, _)) | None => self.file_tree.toggle_expand(),
                    Some((false, path)) => {
                        self.focus_agent_for_path(&path);
                    }
                }
            }
            InputEvent::ScrollUp => {
                if let Some(agent_id) = self.focused_agent_id().cloned() {
                    if let Some(term) = self.terminals.get_mut(agent_id.as_str()) {
                        term.scroll_up();
                    }
                }
            }
            InputEvent::ScrollDown => {
                if let Some(agent_id) = self.focused_agent_id().cloned() {
                    if let Some(term) = self.terminals.get_mut(agent_id.as_str()) {
                        term.scroll_down();
                    }
                }
            }
            InputEvent::SpawnNewAgent => {} // Handled in run_inner (needs NATS)
            InputEvent::EnterCommandMode => {}
            InputEvent::RawInput(_) => {}
        }
    }

    fn move_focus_left(&mut self) {
        match &self.focus {
            FocusTarget::Coordinator => {
                if self.tiling.has_panes() {
                    self.focus = FocusTarget::Agent(AgentId("center".into()));
                } else if self.file_tree_visible {
                    self.focus = FocusTarget::FileTree;
                }
            }
            FocusTarget::Agent(_) => {
                if self.file_tree_visible {
                    self.focus = FocusTarget::FileTree;
                }
            }
            FocusTarget::FileTree => {}
        }
    }

    fn move_focus_right(&mut self) {
        match &self.focus {
            FocusTarget::FileTree => {
                if self.tiling.has_panes() {
                    self.focus = FocusTarget::Agent(AgentId("center".into()));
                } else if self.coordinator_visible {
                    self.focus = FocusTarget::Coordinator;
                }
            }
            FocusTarget::Agent(_) => {
                if self.coordinator_visible {
                    self.focus = FocusTarget::Coordinator;
                }
            }
            FocusTarget::Coordinator => {}
        }
    }

    fn focus_dock_item(&mut self, _n: usize) {
        // TODO: map dock index to agent ID
    }

    /// Get the agent ID that currently has focus (for stdin routing).
    pub fn focused_agent_id(&self) -> Option<&AgentId> {
        match &self.focus {
            FocusTarget::Coordinator => self.coordinator_id.as_ref(),
            FocusTarget::Agent(id) => Some(id),
            FocusTarget::FileTree => None,
        }
    }
}

/// Run the TUI application.
pub async fn run(config: ForgeConfig, workdir: PathBuf) -> Result<()> {
    // Install panic hook that restores the terminal
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = stdout().execute(LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    let result = run_inner(config, workdir).await;

    // Always restore terminal, even on error
    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);

    result
}

async fn run_inner(config: ForgeConfig, workdir: PathBuf) -> Result<()> {
    // Start or connect to the daemon
    let daemon_state = start_or_connect_daemon(&config, &workdir).await?;
    let nats_url = &daemon_state.nats_url;

    // Connect to NATS
    let nats = async_nats::connect(nats_url)
        .await
        .context("Failed to connect to NATS")?;

    // Request session snapshot from daemon
    let snapshot = request_session_snapshot(&nats).await?;

    // Set up terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(config, workdir);
    let mut input_state = input::InputState::new();
    let size = terminal.size()?;

    // Register agents from snapshot
    for agent_info in snapshot.agents {
        let id_str = agent_info.id.as_str().to_string();
        let is_coord = app.coordinator_id.is_none(); // first agent = coordinator
        app.register_agent(agent_info);

        // Size each agent's terminal to its actual pane dimensions
        let (cols, rows) = if is_coord {
            app.coordinator_pane_size(size.width, size.height)
        } else {
            app.center_pane_size(size.width, size.height)
        };
        app.ensure_terminal(&id_str, cols, rows);
    }

    // Restore tiling layout from snapshot (if re-attaching)
    if let Some(ref layout_json) = snapshot.layout {
        if let Some(restored) = TilingLayout::deserialize(layout_json) {
            app.tiling = restored;
            app.needs_resize = true;
        }
    }

    // Replay scrollback for each agent (request buffered output from daemon)
    for (id_str, _) in &app.agents {
        let cmd = CoordinatorCommand::ReadOutput {
            agent_id: AgentId(id_str.clone()),
            last_n_lines: 2000,
        };
        let payload = serde_json::to_vec(&cmd).unwrap();
        if let Ok(reply) = nats
            .request(
                CoordinatorSubjects::command(),
                payload.into(),
            )
            .await
        {
            // Feed the raw output into the terminal emulator for proper ANSI rendering
            if let Some(term) = app.terminals.get_mut(id_str) {
                term.process_bytes(&reply.payload);
            }
        }
    }

    // Track last-known pane sizes to detect when resizes are needed
    let mut last_pane_sizes: HashMap<String, (u16, u16)> = HashMap::new();

    // Subscribe to all agent stdout streams
    let mut stdout_sub = nats.subscribe(AgentSubjects::all_stdout()).await?;

    // Subscribe to agent state changes
    let mut state_sub = nats.subscribe(AgentSubjects::all_states()).await?;

    // Subscribe to filesystem events (for incremental file tree updates)
    let mut fs_sub = nats.subscribe(AgentSubjects::all_fs()).await?;

    // Subscribe to remote directory snapshots from SSH agents
    let mut remote_snapshot_sub = nats.subscribe(AgentSubjects::all_remote_snapshots()).await?;

    // Main loop
    while app.running {
        // Poll NATS for agent stdout (non-blocking drain)
        loop {
            match tokio::time::timeout(Duration::from_millis(1), stdout_sub.next()).await {
                Ok(Some(msg)) => {
                    if let Some(agent_id) = extract_agent_id_from_subject(&msg.subject) {
                        if let Some(term) = app.terminals.get_mut(&agent_id) {
                            term.process_bytes(&msg.payload);
                        }
                    }
                }
                _ => break,
            }
        }

        // Poll NATS for state changes (non-blocking drain)
        loop {
            match tokio::time::timeout(Duration::from_millis(1), state_sub.next()).await {
                Ok(Some(msg)) => {
                    if let Ok(event) = serde_json::from_slice::<AgentStateEvent>(&msg.payload) {
                        let id_str = event.agent_id.as_str().to_string();
                        if let Some(agent) = app.agents.get_mut(&id_str) {
                            agent.state = event.state;
                        } else {
                            let info = AgentInfo {
                                id: event.agent_id.clone(),
                                label: id_str.clone(),
                                command: String::new(),
                                args: Vec::new(),
                                host: forge_core::types::AgentHost::Local,
                                working_dir: String::new(),
                                state: event.state,
                                color: forge_core::types::AgentColor::from_index(app.agents.len()),
                                mcp_capable: false,
                                agent_type: String::new(),
                            };
                            app.register_agent(info);
                            // Terminal will be created + resized by the per-frame size check
                            let term_size = terminal.size().unwrap_or_default();
                            let (acols, arows) = app.center_pane_size(term_size.width, term_size.height);
                            app.ensure_terminal(&id_str, acols, arows);
                        }
                    }
                }
                _ => break,
            }
        }

        // Poll NATS for filesystem events — trigger incremental tree refresh.
        loop {
            match tokio::time::timeout(Duration::from_millis(1), fs_sub.next()).await {
                Ok(Some(msg)) => {
                    if serde_json::from_slice::<FsEvent>(&msg.payload).is_ok() {
                        app.file_tree.refresh(&app.workdir);
                    }
                }
                _ => break,
            }
        }

        // Poll NATS for remote directory snapshots — update remote sections in file tree.
        loop {
            match tokio::time::timeout(Duration::from_millis(1), remote_snapshot_sub.next()).await {
                Ok(Some(msg)) => {
                    if let Ok(snapshot) = serde_json::from_slice::<RemoteSnapshot>(&msg.payload) {
                        let entries = snapshot
                            .entries
                            .into_iter()
                            .map(|e| crate::file_tree::RemoteEntry {
                                name: e.name,
                                path: std::path::PathBuf::from(&e.path),
                                is_dir: e.is_dir,
                                depth: e.depth,
                            })
                            .collect();
                        app.file_tree.add_remote_section(snapshot.host_name, entries);
                    }
                }
                _ => break,
            }
        }

        // Render
        terminal.draw(|frame| {
            ui::render(frame, &app);
        })?;

        // Resize PTYs when layout changes (event-driven, not every frame)
        if app.needs_resize {
            app.needs_resize = false;

            let term_size = terminal.size().unwrap_or_default();
            let area = Rect::new(0, 0, term_size.width, term_size.height);
            let current_sizes = crate::ui::layout::compute_pane_sizes(&app, area);

            for (agent_id_str, &(cols, rows)) in &current_sizes {
                let changed = last_pane_sizes
                    .get(agent_id_str)
                    .map(|&(oc, or)| oc != cols || or != rows)
                    .unwrap_or(true);

                if changed {
                    if let Some(term) = app.terminals.get_mut(agent_id_str) {
                        term.resize(cols, rows);
                    }

                    let cmd = forge_core::protocol::CoordinatorCommand::ResizeAgent {
                        agent_id: AgentId(agent_id_str.clone()),
                        cols,
                        rows,
                    };
                    let payload = serde_json::to_vec(&cmd).unwrap();
                    let _ = nats
                        .publish(
                            forge_core::subjects::CoordinatorSubjects::command(),
                            payload.into(),
                        )
                        .await;
                }
            }

            last_pane_sizes = current_sizes;
        }

        // Handle input
        if event::poll(Duration::from_millis(14))? {
            match event::read()? {
                Event::Key(key) => {
                    if let Some(input_event) = input::map_key_event(key, &mut input_state) {
                        app.command_mode = input_state.command_mode;
                        match &input_event {
                            InputEvent::EnterCommandMode => {
                                // Command mode activated — next key will be interpreted
                                // as a forge command. Nothing to do here.
                            }
                            InputEvent::SpawnNewAgent => {
                                // Send spawn command to daemon via NATS
                                let cmd = forge_core::protocol::CoordinatorCommand::SpawnAgent {
                                    label: format!("agent-{}", app.agents.len()),
                                    agent_type: None,
                                    command: None,
                                    args: None,
                                    host: forge_core::types::AgentHost::Local,
                                    working_dir: app.workdir.to_string_lossy().to_string(),
                                    task: None,
                                };
                                let payload = serde_json::to_vec(&cmd).unwrap();
                                let _ = nats
                                    .publish(
                                        forge_core::subjects::CoordinatorSubjects::command(),
                                        payload.into(),
                                    )
                                    .await;
                            }
                            InputEvent::RawInput(key_event) => {
                                if app.focus == FocusTarget::FileTree {
                                    if let Some(tree_event) = input::map_tree_key(key_event) {
                                        app.handle_input(tree_event);
                                    }
                                } else if let Some(agent_id) = app.focused_agent_id().cloned() {
                                    if let Some(bytes) = key_event_to_bytes(key_event) {
                                        let subject = AgentSubjects::stdin(&agent_id);
                                        let _ = nats.publish(subject, bytes.into()).await;
                                    }
                                }
                            }
                            _ => app.handle_input(input_event),
                        }
                    }
                }
                Event::Resize(_, _) => {
                    app.needs_resize = true;
                }
                _ => {}
            }
        }
    }

    // Publish detach event with current layout so daemon persists it
    let layout_json = app.tiling.serialize();
    let detach_event = UiEvent::Detached { layout: layout_json };
    let payload = serde_json::to_vec(&detach_event).unwrap();
    let _ = nats.publish(SessionSubjects::ui_events(), payload.into()).await;
    let _ = nats.flush().await;

    Ok(())
}

/// Start the daemon or connect to an existing one.
async fn start_or_connect_daemon(config: &ForgeConfig, workdir: &std::path::Path) -> Result<DaemonState> {
    // Check for existing daemon
    if let Some(state) = DaemonState::read()? {
        if state.is_alive() {
            // Verify NATS is reachable
            if async_nats::connect(&state.nats_url).await.is_ok() {
                return Ok(state);
            }
        }
        // Stale state file
        DaemonState::remove();
    }

    // Start forged as a child process
    let forged_path = std::env::current_exe()?
        .parent()
        .unwrap()
        .join("forged");

    if !forged_path.exists() {
        anyhow::bail!(
            "forged binary not found at {}. Build it with: cargo build",
            forged_path.display()
        );
    }

    let mut cmd = tokio::process::Command::new(&forged_path);
    cmd.arg("--workdir").arg(workdir);
    if let Some(config_path) = config_path_if_exists() {
        cmd.arg("--config").arg(config_path);
    }
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().context(format!(
        "Failed to start forged at {}",
        forged_path.display()
    ))?;

    // Wait for the state file to appear.
    // First run may take longer (nats-server auto-download), so wait up to 30s.
    for i in 0..300 {
        // Check if the child exited early (crashed)
        if let Some(status) = child.try_wait()? {
            let mut stderr_msg = String::new();
            if let Some(mut stderr) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let _ = stderr.read_to_string(&mut stderr_msg).await;
            }
            let log_path = ForgeConfig::data_dir().join("forged.log");
            anyhow::bail!(
                "Daemon exited with {status}\n\
                 stderr: {}\n\
                 Check log: {}",
                if stderr_msg.is_empty() { "(empty)" } else { &stderr_msg },
                log_path.display()
            );
        }

        tokio::time::sleep(Duration::from_millis(100)).await;

        if let Some(state) = DaemonState::read()? {
            if state.is_alive() {
                if async_nats::connect(&state.nats_url).await.is_ok() {
                    // Detach — don't hold the child handle
                    drop(child);
                    return Ok(state);
                }
            }
        }

        // Print progress on first launch (nats-server download may take a while)
        if i == 30 {
            eprintln!("Starting forge daemon (first run may download nats-server)...");
        }
    }

    // If we get here, daemon didn't write state file in 30s
    let _ = child.kill().await;
    let log_path = ForgeConfig::data_dir().join("forged.log");
    anyhow::bail!(
        "Daemon failed to start within 30 seconds.\nCheck log: {}",
        log_path.display()
    )
}

fn config_path_if_exists() -> Option<PathBuf> {
    let path = ForgeConfig::default_path();
    if path.exists() { Some(path) } else { None }
}

/// Request a session snapshot from the daemon.
async fn request_session_snapshot(nats: &async_nats::Client) -> Result<SessionSnapshot> {
    let reply = nats
        .request(SessionSubjects::state(), bytes::Bytes::new())
        .await
        .context("Failed to get session snapshot from daemon")?;

    let snapshot: SessionSnapshot = serde_json::from_slice(&reply.payload)
        .context("Failed to parse session snapshot")?;

    Ok(snapshot)
}

/// Extract agent ID from a NATS subject like "forge.agent.{id}.stdout"
fn extract_agent_id_from_subject(subject: &str) -> Option<String> {
    let parts: Vec<&str> = subject.split('.').collect();
    if parts.len() >= 4 && parts[0] == "forge" && parts[1] == "agent" {
        Some(parts[2].to_string())
    } else {
        None
    }
}

/// Convert a crossterm KeyEvent into bytes to send to a PTY.
fn key_event_to_bytes(key: &KeyEvent) -> Option<Vec<u8>> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let byte = (c as u8).wrapping_sub(b'a').wrapping_add(1);
                if byte <= 26 {
                    Some(vec![byte])
                } else {
                    Some(c.to_string().into_bytes())
                }
            } else {
                Some(c.to_string().into_bytes())
            }
        }
        KeyCode::Enter => Some(vec![b'\r']),
        KeyCode::Backspace => Some(vec![0x7f]),
        KeyCode::Tab => Some(vec![b'\t']),
        KeyCode::Esc => Some(vec![0x1b]),
        KeyCode::Up => Some(b"\x1b[A".to_vec()),
        KeyCode::Down => Some(b"\x1b[B".to_vec()),
        KeyCode::Right => Some(b"\x1b[C".to_vec()),
        KeyCode::Left => Some(b"\x1b[D".to_vec()),
        KeyCode::Home => Some(b"\x1b[H".to_vec()),
        KeyCode::End => Some(b"\x1b[F".to_vec()),
        KeyCode::PageUp => Some(b"\x1b[5~".to_vec()),
        KeyCode::PageDown => Some(b"\x1b[6~".to_vec()),
        KeyCode::Delete => Some(b"\x1b[3~".to_vec()),
        KeyCode::Insert => Some(b"\x1b[2~".to_vec()),
        KeyCode::F(n) => {
            let seq = match n {
                1 => "\x1bOP",
                2 => "\x1bOQ",
                3 => "\x1bOR",
                4 => "\x1bOS",
                5 => "\x1b[15~",
                6 => "\x1b[17~",
                7 => "\x1b[18~",
                8 => "\x1b[19~",
                9 => "\x1b[20~",
                10 => "\x1b[21~",
                11 => "\x1b[23~",
                12 => "\x1b[24~",
                _ => return None,
            };
            Some(seq.as_bytes().to_vec())
        }
        _ => None,
    }
}
