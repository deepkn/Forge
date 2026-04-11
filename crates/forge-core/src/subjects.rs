//! NATS subject definitions for the Forge message bus.
//!
//! All inter-component communication flows through these subjects.

use crate::types::AgentId;

/// Root prefix for all Forge NATS subjects.
const PREFIX: &str = "forge";

/// Agent-scoped subjects.
pub struct AgentSubjects;

impl AgentSubjects {
    /// Raw PTY stdout bytes from an agent.
    pub fn stdout(id: &AgentId) -> String {
        format!("{PREFIX}.agent.{id}.stdout")
    }

    /// Input destined for an agent's PTY stdin.
    pub fn stdin(id: &AgentId) -> String {
        format!("{PREFIX}.agent.{id}.stdin")
    }

    /// Agent state changes (AgentState transitions).
    pub fn state(id: &AgentId) -> String {
        format!("{PREFIX}.agent.{id}.state")
    }

    /// File system events from an agent's working directory.
    pub fn fs(id: &AgentId) -> String {
        format!("{PREFIX}.agent.{id}.fs")
    }

    /// Subscribe to all events for a specific agent.
    pub fn all(id: &AgentId) -> String {
        format!("{PREFIX}.agent.{id}.>")
    }

    /// Subscribe to all agent events (wildcard).
    pub fn all_agents() -> String {
        format!("{PREFIX}.agent.>")
    }

    /// Subscribe to all agent state changes.
    pub fn all_states() -> String {
        format!("{PREFIX}.agent.*.state")
    }

    /// Subscribe to all agent stdout streams.
    pub fn all_stdout() -> String {
        format!("{PREFIX}.agent.*.stdout")
    }

    /// Subscribe to all agent fs event streams.
    pub fn all_fs() -> String {
        format!("{PREFIX}.agent.*.fs")
    }

    /// MCP tool calls from an agent.
    pub fn mcp_call(id: &AgentId) -> String {
        format!("{PREFIX}.agent.{id}.mcp.call")
    }

    /// MCP tool results to an agent.
    pub fn mcp_result(id: &AgentId) -> String {
        format!("{PREFIX}.agent.{id}.mcp.result")
    }

    /// Subscribe to all MCP calls from all agents.
    pub fn all_mcp_calls() -> String {
        format!("{PREFIX}.agent.*.mcp.call")
    }
}

/// Coordinator inbox subjects.
pub struct CoordinatorInbox;

impl CoordinatorInbox {
    /// Notifications delivered to the coordinator.
    pub fn notifications() -> String {
        format!("{PREFIX}.coord.notifications")
    }
}

/// Coordinator subjects.
pub struct CoordinatorSubjects;

impl CoordinatorSubjects {
    /// Orchestration commands (spawn, kill, assign).
    pub fn command() -> String {
        format!("{PREFIX}.coordinator.command")
    }

    /// Current execution plan.
    pub fn plan() -> String {
        format!("{PREFIX}.coordinator.plan")
    }
}

/// File lock subjects.
pub struct FileLockSubjects;

impl FileLockSubjects {
    /// Lock acquisition/release requests (request-reply pattern).
    pub fn request() -> String {
        format!("{PREFIX}.file.lock.request")
    }

    /// Lock state broadcasts (for UI updates).
    pub fn state() -> String {
        format!("{PREFIX}.file.lock.state")
    }
}

/// Session subjects.
pub struct SessionSubjects;

impl SessionSubjects {
    /// Session-level state changes.
    pub fn state() -> String {
        format!("{PREFIX}.session.state")
    }

    /// UI events (focus change, layout change).
    pub fn ui_events() -> String {
        format!("{PREFIX}.session.ui.events")
    }
}
