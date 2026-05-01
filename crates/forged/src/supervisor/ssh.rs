//! SSH PTY supervisor — spawns an agent CLI on a remote machine via SSH.
//!
//! Uses SSH with:
//! - `-t` for remote PTY allocation
//! - `-R` for reverse tunnel so the remote agent can reach the local NATS server
//!
//! The SSH process itself is a local child process whose stdin/stdout
//! map to the remote PTY, reusing the same NATS bridging pattern as LocalSupervisor.

use super::Supervisor;
use anyhow::{Context, Result};
use futures_util::StreamExt;
use forge_core::protocol::{AgentStateEvent, RemoteSnapshot, RemoteSnapshotEntry};
use forge_core::subjects::AgentSubjects;
use forge_core::types::{AgentId, AgentState};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

/// Port range for SSH reverse tunnels.
const TUNNEL_PORT_START: u16 = 14200;
const TUNNEL_PORT_END: u16 = 14300;

pub struct SshSupervisor {
    agent_id: AgentId,
    host: String,
    command: String,
    args: Vec<String>,
    working_dir: String,
    nats_port: u16,
    tunnel_port: u16,
    ssh_child: Arc<Mutex<Option<Child>>>,
    stdin_writer: Arc<Mutex<Option<tokio::process::ChildStdin>>>,
    stdout_task: Option<JoinHandle<()>>,
    health_task: Option<JoinHandle<()>>,
    snapshot_task: Option<JoinHandle<()>>,
    alive: Arc<AtomicBool>,
}

impl SshSupervisor {
    pub fn new(
        agent_id: AgentId,
        host: String,
        command: String,
        args: Vec<String>,
        working_dir: String,
        nats_port: u16,
    ) -> Self {
        Self {
            agent_id,
            host,
            command,
            args,
            working_dir,
            nats_port,
            tunnel_port: 0,
            ssh_child: Arc::new(Mutex::new(None)),
            stdin_writer: Arc::new(Mutex::new(None)),
            stdout_task: None,
            health_task: None,
            snapshot_task: None,
            alive: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Start the SSH connection and wire I/O to NATS.
    pub async fn start_with_nats(&mut self, nats: async_nats::Client) -> Result<()> {
        // Find an available tunnel port
        self.tunnel_port = find_available_tunnel_port()?;

        // Auto-deploy forge-mcp if needed
        self.ensure_forge_mcp_deployed().await?;

        // Build the SSH command
        let remote_cmd = self.build_remote_command();
        let mut ssh_cmd = Command::new("ssh");
        ssh_cmd
            .arg("-t") // Force PTY allocation
            .arg("-o").arg("StrictHostKeyChecking=accept-new")
            .arg("-o").arg("ServerAliveInterval=15")
            .arg("-o").arg("ServerAliveCountMax=3")
            .arg("-R").arg(format!("{}:localhost:{}", self.tunnel_port, self.nats_port))
            .arg(&self.host)
            .arg(&remote_cmd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let mut child = ssh_cmd
            .spawn()
            .with_context(|| format!("Failed to spawn SSH to {}", self.host))?;

        // Extract stdin/stdout handles
        let stdin = child.stdin.take().context("Failed to take SSH stdin")?;
        let stdout = child.stdout.take().context("Failed to take SSH stdout")?;

        self.stdin_writer = Arc::new(Mutex::new(Some(stdin)));
        self.ssh_child = Arc::new(Mutex::new(Some(child)));
        self.alive.store(true, Ordering::Relaxed);

        // Spawn stdout → NATS bridge (same pattern as LocalSupervisor)
        let agent_id = self.agent_id.clone();
        let alive = self.alive.clone();
        let stdout_subject = AgentSubjects::stdout(&agent_id);
        let state_subject = AgentSubjects::state(&agent_id);
        let nats_for_stdout = nats.clone();

        self.stdout_task = Some(tokio::spawn(async move {
            let mut reader = stdout;
            let mut buf = [0u8; 4096];

            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => {
                        // SSH connection closed
                        alive.store(false, Ordering::Relaxed);
                        let event = AgentStateEvent {
                            agent_id: agent_id.clone(),
                            state: AgentState::Done,
                            message: Some("SSH connection closed".to_string()),
                        };
                        let payload = serde_json::to_vec(&event).unwrap();
                        let _ = nats_for_stdout
                            .publish(state_subject, payload.into())
                            .await;
                        break;
                    }
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        let _ = nats_for_stdout
                            .publish(stdout_subject.clone(), data.into())
                            .await;
                    }
                    Err(e) => {
                        error!("SSH stdout read error for agent {}: {e}", agent_id);
                        alive.store(false, Ordering::Relaxed);
                        break;
                    }
                }
            }
        }));

        // Subscribe to stdin from NATS
        let writer = self.stdin_writer.clone();
        let stdin_subject = AgentSubjects::stdin(&self.agent_id);
        let nats_for_snapshot = nats.clone();
        tokio::spawn(async move {
            let mut sub = match nats.subscribe(stdin_subject).await {
                Ok(s) => s,
                Err(e) => {
                    error!("Failed to subscribe to SSH agent stdin: {e}");
                    return;
                }
            };
            while let Some(msg) = sub.next().await {
                let mut guard = writer.lock().await;
                if let Some(ref mut stdin) = *guard {
                    let _ = stdin.write_all(&msg.payload).await;
                    let _ = stdin.flush().await;
                }
            }
        });

        // Spawn SSH health monitor
        let ssh_child_for_health = self.ssh_child.clone();
        let alive_for_health = self.alive.clone();
        let host = self.host.clone();
        self.health_task = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;

                if !alive_for_health.load(Ordering::Relaxed) {
                    break;
                }

                let mut guard = ssh_child_for_health.lock().await;
                if let Some(ref mut child) = *guard {
                    match child.try_wait() {
                        Ok(Some(status)) => {
                            warn!("SSH to {} exited with status: {}", host, status);
                            alive_for_health.store(false, Ordering::Relaxed);
                            break;
                        }
                        Ok(None) => {} // Still running
                        Err(e) => {
                            error!("Failed to check SSH health for {}: {e}", host);
                            break;
                        }
                    }
                }
            }
        }));

        info!(
            "SSH supervisor started for agent {} on {} (tunnel port {})",
            self.agent_id, self.host, self.tunnel_port
        );

        // Start remote snapshot + periodic polling
        self.snapshot_task = Some(self.start_snapshot_task(nats_for_snapshot));

        Ok(())
    }

    /// Spawn a task that takes an initial remote directory snapshot then polls every 30s.
    fn start_snapshot_task(&self, nats: async_nats::Client) -> JoinHandle<()> {
        let host = self.host.clone();
        let working_dir = self.working_dir.clone();
        let agent_id = self.agent_id.clone();
        let alive = self.alive.clone();
        let snapshot_subject = AgentSubjects::remote_snapshot(&agent_id);

        tokio::spawn(async move {
            // Small delay to let SSH connection stabilize
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;

            let mut first = true;
            loop {
                if !alive.load(Ordering::Relaxed) {
                    break;
                }

                match run_remote_find(&host, &working_dir).await {
                    Ok(entries) => {
                        let snapshot = RemoteSnapshot {
                            agent_id: agent_id.clone(),
                            host_name: host.clone(),
                            entries,
                        };
                        match serde_json::to_vec(&snapshot) {
                            Ok(payload) => {
                                let _ = nats
                                    .publish(snapshot_subject.clone(), payload.into())
                                    .await;
                            }
                            Err(e) => error!("Failed to serialize remote snapshot: {e}"),
                        }
                    }
                    Err(e) => {
                        if first {
                            warn!("Initial remote snapshot for {} failed: {e}", host);
                        }
                    }
                }

                first = false;
                // Poll every 30 seconds
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            }
        })
    }

    /// Build the command string to run on the remote host.
    fn build_remote_command(&self) -> String {
        let nats_url = format!("nats://localhost:{}", self.tunnel_port);
        let args_str = self.args.join(" ");

        format!(
            "cd {} && FORGE_NATS_URL={} {} {}",
            shell_escape(&self.working_dir),
            shell_escape(&nats_url),
            shell_escape(&self.command),
            args_str
        )
    }

    /// Check if forge-mcp exists on the remote, deploy if not.
    async fn ensure_forge_mcp_deployed(&self) -> Result<()> {
        // Check if forge-mcp exists on remote
        let check = Command::new("ssh")
            .arg(&self.host)
            .arg("which forge-mcp 2>/dev/null || echo __NOT_FOUND__")
            .output()
            .await
            .context("Failed to check forge-mcp on remote")?;

        let output = String::from_utf8_lossy(&check.stdout);
        if output.contains("__NOT_FOUND__") {
            info!("forge-mcp not found on {}, deploying...", self.host);

            // Find local forge-mcp binary
            let local_bin = std::env::current_exe()?
                .parent()
                .unwrap()
                .join("forge-mcp");

            if !local_bin.exists() {
                anyhow::bail!(
                    "Local forge-mcp binary not found at {}. Build it first.",
                    local_bin.display()
                );
            }

            // Create remote directory and copy binary
            let _ = Command::new("ssh")
                .arg(&self.host)
                .arg("mkdir -p ~/.local/bin")
                .status()
                .await;

            let scp_status = Command::new("scp")
                .arg(local_bin.to_string_lossy().as_ref())
                .arg(format!("{}:~/.local/bin/forge-mcp", self.host))
                .status()
                .await
                .context("Failed to scp forge-mcp to remote")?;

            if !scp_status.success() {
                anyhow::bail!("scp of forge-mcp to {} failed", self.host);
            }

            // Make executable
            let _ = Command::new("ssh")
                .arg(&self.host)
                .arg("chmod +x ~/.local/bin/forge-mcp")
                .status()
                .await;

            info!("forge-mcp deployed to {}", self.host);
        }

        Ok(())
    }
}

impl Supervisor for SshSupervisor {
    fn start(&mut self) -> Result<()> {
        anyhow::bail!("Use start_with_nats() for SSH supervisors")
    }

    fn send_input(&self, data: &[u8]) -> Result<()> {
        let writer = self.stdin_writer.clone();
        let data = data.to_vec();
        tokio::spawn(async move {
            let mut guard = writer.lock().await;
            if let Some(ref mut stdin) = *guard {
                let _ = stdin.write_all(&data).await;
                let _ = stdin.flush().await;
            }
        });
        Ok(())
    }

    fn resize(&self, _rows: u16, _cols: u16) -> Result<()> {
        // SSH PTY resize is handled by the terminal emulator on the remote side.
        // We'd need to send a SIGWINCH or use SSH window change request.
        // For now, this is a no-op — the remote terminal adapts on its own.
        Ok(())
    }

    fn kill(&mut self) -> Result<()> {
        self.alive.store(false, Ordering::Relaxed);
        let child = self.ssh_child.clone();
        tokio::spawn(async move {
            let mut guard = child.lock().await;
            if let Some(ref mut child) = *guard {
                let _ = child.kill().await;
            }
        });
        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    fn agent_id(&self) -> &AgentId {
        &self.agent_id
    }
}

/// Run `find` on the remote host and parse output into snapshot entries.
async fn run_remote_find(host: &str, working_dir: &str) -> Result<Vec<RemoteSnapshotEntry>> {
    // find <dir> -maxdepth 4 -not -path '*/.*' printing: type, depth(from wd), path
    // We count depth by stripping the working_dir prefix and counting slashes
    let find_cmd = format!(
        "find {} -maxdepth 4 -not -path '*/.*' -printf '%y\\t%p\\n' 2>/dev/null | head -500",
        shell_escape(working_dir)
    );

    let output = Command::new("ssh")
        .arg("-o").arg("StrictHostKeyChecking=accept-new")
        .arg("-o").arg("BatchMode=yes")
        .arg(host)
        .arg(&find_cmd)
        .output()
        .await
        .context("Failed to run remote find")?;

    if !output.status.success() && output.stdout.is_empty() {
        anyhow::bail!("Remote find failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    let base = working_dir.trim_end_matches('/');
    let mut entries = Vec::new();

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.splitn(2, '\t').collect();
        if parts.len() != 2 {
            continue;
        }
        let ftype = parts[0];
        let path = parts[1].trim_end_matches('/');

        // Skip the working dir itself
        if path == base {
            continue;
        }

        let is_dir = ftype == "d";
        let relative = path.strip_prefix(base).unwrap_or(path).trim_start_matches('/');
        let depth = relative.chars().filter(|&c| c == '/').count();
        let name = std::path::Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(relative)
            .to_string();

        entries.push(RemoteSnapshotEntry {
            name,
            path: path.to_string(),
            is_dir,
            depth,
        });
    }

    Ok(entries)
}

/// Find an available port in the tunnel range.
fn find_available_tunnel_port() -> Result<u16> {
    for port in TUNNEL_PORT_START..TUNNEL_PORT_END {
        if std::net::TcpListener::bind(format!("127.0.0.1:{}", port)).is_ok() {
            return Ok(port);
        }
    }
    anyhow::bail!(
        "No available tunnel ports in range {}-{}",
        TUNNEL_PORT_START,
        TUNNEL_PORT_END
    )
}

/// Simple shell escaping for remote command strings.
fn shell_escape(s: &str) -> String {
    if s.chars().all(|c| c.is_alphanumeric() || c == '/' || c == '.' || c == '-' || c == '_' || c == ':') {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ssh_command_construction() {
        let sup = SshSupervisor {
            agent_id: AgentId("test-01".to_string()),
            host: "dev-server".to_string(),
            command: "claude".to_string(),
            args: vec!["--mcp-config".to_string(), "/tmp/mcp.json".to_string()],
            working_dir: "/home/user/project".to_string(),
            nats_port: 4222,
            tunnel_port: 14200,
            ssh_child: Arc::new(Mutex::new(None)),
            stdin_writer: Arc::new(Mutex::new(None)),
            stdout_task: None,
            health_task: None,
            snapshot_task: None,
            alive: Arc::new(AtomicBool::new(false)),
        };

        let cmd = sup.build_remote_command();
        assert!(cmd.contains("cd /home/user/project"));
        assert!(cmd.contains("FORGE_NATS_URL=nats://localhost:14200"));
        assert!(cmd.contains("claude"));
    }

    #[test]
    fn test_shell_escape() {
        assert_eq!(shell_escape("simple"), "simple");
        assert_eq!(shell_escape("/path/to/file"), "/path/to/file");
        assert_eq!(shell_escape("has space"), "'has space'");
        assert_eq!(shell_escape("it's"), "\"it's\"".replace('"', "'").replace("'", "'\\''").replace("'\\''s'", "'it'\\''s'"));
    }

    #[test]
    fn test_tunnel_port_finding() {
        // This should succeed since ports in the 14200-14300 range are unlikely to be in use
        let port = find_available_tunnel_port();
        assert!(port.is_ok());
        let port = port.unwrap();
        assert!(port >= TUNNEL_PORT_START && port < TUNNEL_PORT_END);
    }
}
