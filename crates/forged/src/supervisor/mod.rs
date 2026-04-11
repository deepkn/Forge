pub mod local;

use anyhow::Result;
use forge_core::types::AgentId;

/// Trait for subagent supervisors (local and remote).
pub trait Supervisor: Send {
    /// Start the agent process and begin I/O bridging.
    fn start(&mut self) -> Result<()>;

    /// Send input bytes to the agent's stdin.
    fn send_input(&self, data: &[u8]) -> Result<()>;

    /// Resize the agent's terminal.
    fn resize(&self, rows: u16, cols: u16) -> Result<()>;

    /// Kill the agent process.
    fn kill(&mut self) -> Result<()>;

    /// Check if the agent process is still running.
    fn is_alive(&self) -> bool;

    /// Get the agent's ID.
    fn agent_id(&self) -> &AgentId;
}
