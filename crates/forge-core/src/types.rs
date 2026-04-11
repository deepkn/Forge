use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a subagent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string()[..8].to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string()[..8].to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Current state of a subagent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Starting,
    Running,
    Waiting,
    Done,
    Error,
}

impl fmt::Display for AgentState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Starting => write!(f, "starting"),
            Self::Running => write!(f, "running"),
            Self::Waiting => write!(f, "waiting"),
            Self::Done => write!(f, "done"),
            Self::Error => write!(f, "error"),
        }
    }
}

/// Where a subagent is running.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentHost {
    Local,
    Remote { name: String },
}

impl fmt::Display for AgentHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => write!(f, "local"),
            Self::Remote { name } => write!(f, "{name}"),
        }
    }
}

/// File lock mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LockMode {
    Read,
    Write,
}

impl fmt::Display for LockMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read => write!(f, "R"),
            Self::Write => write!(f, "W"),
        }
    }
}

/// Metadata for a registered subagent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInfo {
    pub id: AgentId,
    pub label: String,
    pub command: String,
    pub args: Vec<String>,
    pub host: AgentHost,
    pub working_dir: String,
    pub state: AgentState,
    pub color: AgentColor,
    /// Whether this agent has MCP capabilities.
    #[serde(default)]
    pub mcp_capable: bool,
    /// Agent type name from registry.
    #[serde(default)]
    pub agent_type: String,
}

/// Color assigned to a subagent for UI identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentColor {
    Blue,
    Green,
    Yellow,
    Red,
    Magenta,
    Cyan,
    Orange,
    Purple,
}

impl AgentColor {
    /// Assigns a color based on agent index (cycles through palette).
    pub fn from_index(idx: usize) -> Self {
        const PALETTE: &[AgentColor] = &[
            AgentColor::Blue,
            AgentColor::Green,
            AgentColor::Yellow,
            AgentColor::Magenta,
            AgentColor::Cyan,
            AgentColor::Orange,
            AgentColor::Purple,
            AgentColor::Red,
        ];
        PALETTE[idx % PALETTE.len()]
    }
}
