//! Local PTY supervisor — spawns an agent CLI in a local pseudo-terminal.

use super::Supervisor;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use forge_core::protocol::AgentStateEvent;
use forge_core::subjects::AgentSubjects;
use forge_core::types::{AgentId, AgentState};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tracing::{error, info};

pub struct LocalSupervisor {
    agent_id: AgentId,
    command: String,
    args: Vec<String>,
    working_dir: String,
    initial_cols: u16,
    initial_rows: u16,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    stdout_task: Option<JoinHandle<()>>,
    alive: Arc<std::sync::atomic::AtomicBool>,
}

impl LocalSupervisor {
    pub fn new(
        agent_id: AgentId,
        command: String,
        args: Vec<String>,
        working_dir: String,
    ) -> Self {
        Self {
            agent_id,
            command,
            args,
            working_dir,
            initial_cols: 120,
            initial_rows: 40,
            master: None,
            writer: None,
            stdout_task: None,
            alive: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Set initial PTY dimensions (call before start_with_nats).
    pub fn set_size(&mut self, cols: u16, rows: u16) {
        self.initial_cols = cols;
        self.initial_rows = rows;
    }

    /// Start the PTY and wire I/O to NATS.
    pub fn start_with_nats(&mut self, nats: async_nats::Client) -> Result<()> {
        let pty_system = native_pty_system();

        let pty = pty_system
            .openpty(PtySize {
                rows: self.initial_rows,
                cols: self.initial_cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to open PTY")?;

        let mut cmd = CommandBuilder::new(&self.command);
        cmd.args(&self.args);
        cmd.cwd(&self.working_dir);

        let _child = pty.slave.spawn_command(cmd).context("Failed to spawn agent process")?;

        // Extract writer and reader before wrapping master in Arc<Mutex>
        let writer = Arc::new(Mutex::new(
            pty.master.take_writer().context("Failed to take PTY writer")?,
        ));
        let reader = pty.master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        let master = Arc::new(Mutex::new(pty.master));

        self.master = Some(master);
        self.writer = Some(writer);
        self.alive
            .store(true, std::sync::atomic::Ordering::Relaxed);

        // Spawn stdout → NATS bridge
        let agent_id = self.agent_id.clone();
        let alive = self.alive.clone();
        let stdout_subject = AgentSubjects::stdout(&agent_id);
        let state_subject = AgentSubjects::state(&agent_id);
        let nats_for_stdin = nats.clone();

        self.stdout_task = Some(tokio::task::spawn_blocking(move || {
            let mut reader = reader;
            let mut buf = [0u8; 4096];
            let rt = tokio::runtime::Handle::current();

            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        // PTY closed — agent exited
                        alive.store(false, std::sync::atomic::Ordering::Relaxed);
                        let event = AgentStateEvent {
                            agent_id: agent_id.clone(),
                            state: AgentState::Done,
                            message: Some("Agent process exited".to_string()),
                        };
                        let payload = serde_json::to_vec(&event).unwrap();
                        let nats = nats.clone();
                        let subject = state_subject.clone();
                        rt.spawn(async move {
                            let _ = nats.publish(subject, payload.into()).await;
                        });
                        break;
                    }
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        let nats = nats.clone();
                        let subject = stdout_subject.clone();
                        rt.spawn(async move {
                            let _ = nats.publish(subject, data.into()).await;
                        });
                    }
                    Err(e) => {
                        error!("PTY read error for agent {}: {e}", agent_id);
                        alive.store(false, std::sync::atomic::Ordering::Relaxed);
                        break;
                    }
                }
            }
        }));

        // Subscribe to stdin from NATS
        let writer_clone = self.writer.clone().unwrap();
        let stdin_subject = AgentSubjects::stdin(&self.agent_id);
        tokio::spawn(async move {
            let mut sub = match nats_for_stdin.subscribe(stdin_subject).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to subscribe to agent stdin: {e}");
                    return;
                }
            };
            while let Some(msg) = sub.next().await {
                if let Ok(mut w) = writer_clone.lock() {
                    let _ = w.write_all(&msg.payload);
                    let _ = w.flush();
                }
            }
        });

        info!(
            "Local supervisor started for agent {} ({})",
            self.agent_id, self.command
        );
        Ok(())
    }
}

impl Supervisor for LocalSupervisor {
    fn start(&mut self) -> Result<()> {
        // start_with_nats should be used instead
        anyhow::bail!("Use start_with_nats() for local supervisors")
    }

    fn send_input(&self, data: &[u8]) -> Result<()> {
        if let Some(writer) = &self.writer {
            let mut w = writer.lock().unwrap();
            w.write_all(data)?;
            w.flush()?;
        }
        Ok(())
    }

    fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if let Some(master) = &self.master {
            let m = master.lock().unwrap();
            m.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
        }
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        self.alive
            .store(false, std::sync::atomic::Ordering::Relaxed);
        // Dropping the master PTY will send SIGHUP to the child
        self.master.take();
        self.writer.take();
        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.alive.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
}
