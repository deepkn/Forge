//! Message types exchanged over the NATS message bus.

use crate::types::{AgentHost, AgentId, AgentState, LockMode};
use serde::{Deserialize, Serialize};

// ─── Coordinator Commands ───────────────────────────────────────────

/// Commands sent to the orchestration engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum CoordinatorCommand {
    /// Spawn a new subagent.
    SpawnAgent {
        label: String,
        /// Agent type name from registry (e.g., "claude", "codex", "shell").
        /// If None, uses the default from config.
        agent_type: Option<String>,
        /// Override command (ignores registry).
        command: Option<String>,
        /// Override args.
        args: Option<Vec<String>>,
        host: AgentHost,
        working_dir: String,
        /// Initial task/prompt to inject into the agent.
        task: Option<String>,
    },
    /// Kill a running subagent.
    KillAgent { agent_id: AgentId },
    /// Send a text message into an agent's PTY stdin.
    SendMessage { agent_id: AgentId, message: String },
    /// Read recent output from an agent.
    ReadOutput {
        agent_id: AgentId,
        last_n_lines: usize,
    },
    /// Force-release a file lock.
    ForceReleaseLock { path: String },
    /// Resize an agent's PTY.
    ResizeAgent {
        agent_id: AgentId,
        cols: u16,
        rows: u16,
    },
    /// Graceful shutdown.
    Shutdown,
}

// ─── Agent State Events ─────────────────────────────────────────────

/// Published when an agent's state changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentStateEvent {
    pub agent_id: AgentId,
    pub state: AgentState,
    pub message: Option<String>,
}

// ─── File System Events ─────────────────────────────────────────────

/// Published when an agent's working directory changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsEvent {
    pub agent_id: AgentId,
    pub action: FsAction,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FsAction {
    Create,
    Modify,
    Delete,
    Rename { from: String },
}

// ─── File Lock Protocol ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LockRequest {
    Acquire {
        agent_id: AgentId,
        path: String,
        mode: LockMode,
    },
    Release {
        agent_id: AgentId,
        path: String,
    },
    ForceRelease {
        path: String,
    },
    Query,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LockResponse {
    Granted { path: String, mode: LockMode },
    Denied { path: String, reason: String },
    Released { path: String },
    State { locks: Vec<LockEntry> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub path: String,
    pub agent_id: AgentId,
    pub mode: LockMode,
    pub acquired_at: String,
}

// ─── MCP Tool Calls (subagent → forged) ─────────────────────────────

/// MCP tool calls from agents, published to forge.agent.{id}.mcp.call
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "tool", rename_all = "snake_case")]
pub enum McpToolCall {
    /// Report status change.
    ForgeReportStatus {
        status: String,
        message: String,
    },
    /// Notify task completion.
    ForgeNotifyDone {
        summary: String,
        files_changed: Option<Vec<String>>,
    },
    /// Send a message to the coordinator.
    ForgeSendToCoordinator {
        message: String,
    },
    /// Request input from user/coordinator (blocks until response).
    ForgeRequestInput {
        prompt: String,
    },
    /// Acquire a file lock.
    ForgeLockFile {
        path: String,
        mode: LockMode,
    },
    /// Release a file lock.
    ForgeUnlockFile {
        path: String,
    },
    /// Spawn a new sibling subagent.
    ForgeSpawnAgent {
        task: String,
        label: Option<String>,
        agent_type: Option<String>,
        working_dir: Option<String>,
    },
    /// List all agents.
    ForgeListAgents,
    /// Read output from another agent.
    ForgeReadOutput {
        agent_id: String,
        last_n_lines: Option<usize>,
    },
    /// Send message to another agent.
    ForgeSendMessage {
        agent_id: String,
        message: String,
    },
}

/// MCP tool call result, published to forge.agent.{id}.mcp.result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpToolResult {
    pub call_id: String,
    pub success: bool,
    pub result: serde_json::Value,
}

// ─── Coordinator Inbox ──────────────────────────────────────────────

/// Messages delivered to the coordinator's inbox (from subagents).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorNotification {
    pub from_agent_id: AgentId,
    pub from_label: String,
    pub notification_type: NotificationType,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationType {
    Done,
    Status,
    Message,
    InputRequest,
}

// ─── Session Events ─────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UiEvent {
    FocusChanged { agent_id: AgentId },
    LayoutChanged { layout: String },
    PaneClosed { agent_id: AgentId },
    /// TUI is detaching — daemon should persist the layout.
    Detached { layout: Option<String> },
}

// ─── Remote FS Snapshot ────────────────────────────────────────────

/// A snapshot of a remote agent's working directory tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSnapshot {
    pub agent_id: crate::types::AgentId,
    /// Host name (SSH alias or IP) for the remote agent.
    pub host_name: String,
    /// Files/directories found under the working directory.
    pub entries: Vec<RemoteSnapshotEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteSnapshotEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub depth: usize,
}

// ─── Daemon ↔ TUI handshake ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSnapshot {
    pub session_id: String,
    pub agents: Vec<crate::types::AgentInfo>,
    pub locks: Vec<LockEntry>,
    pub nats_url: String,
    /// Serialized tiling layout JSON (for re-attach).
    #[serde(default)]
    pub layout: Option<String>,
}
