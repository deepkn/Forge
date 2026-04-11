use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Top-level configuration, loaded from ~/.config/forge/config.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ForgeConfig {
    pub agent: AgentConfig,
    pub agents: AgentRegistry,
    pub remote: RemoteConfig,
    pub permissions: PermissionsConfig,
    pub ui: UiConfig,
    pub session: SessionConfig,
    pub locks: LockConfig,
    pub nats: NatsConfig,
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            agent: AgentConfig::default(),
            agents: AgentRegistry::default(),
            remote: RemoteConfig::default(),
            permissions: PermissionsConfig::default(),
            ui: UiConfig::default(),
            session: SessionConfig::default(),
            locks: LockConfig::default(),
            nats: NatsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Default agent type name (must exist in agents registry).
    pub command: String,
    /// Default arguments passed to the agent CLI.
    pub args: Vec<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            command: "claude".to_string(),
            args: vec![],
        }
    }
}

/// Known agent type with its capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentType {
    /// Command to run.
    pub command: String,
    /// Default arguments.
    #[serde(default)]
    pub args: Vec<String>,
    /// Whether this agent supports MCP.
    #[serde(default = "default_true")]
    pub mcp: bool,
    /// CLI flag to pass MCP config file path (e.g., "--mcp-config").
    #[serde(default)]
    pub mcp_config_flag: Option<String>,
    /// CLI flag for direct prompt injection (e.g., "-p").
    #[serde(default)]
    pub prompt_flag: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Registry of known agent types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRegistry {
    #[serde(flatten)]
    pub types: HashMap<String, AgentType>,
}

impl Default for AgentRegistry {
    fn default() -> Self {
        let mut types = HashMap::new();
        types.insert(
            "claude".to_string(),
            AgentType {
                command: "claude".to_string(),
                args: vec![],
                mcp: true,
                mcp_config_flag: Some("--mcp-config".to_string()),
                prompt_flag: Some("-p".to_string()),
            },
        );
        types.insert(
            "codex".to_string(),
            AgentType {
                command: "codex".to_string(),
                args: vec![],
                mcp: true,
                mcp_config_flag: Some("--mcp-config".to_string()),
                prompt_flag: None,
            },
        );
        types.insert(
            "shell".to_string(),
            AgentType {
                command: "bash".to_string(),
                args: vec![],
                mcp: false,
                mcp_config_flag: None,
                prompt_flag: None,
            },
        );
        Self { types }
    }
}

impl AgentRegistry {
    /// Look up an agent type by name. Falls back to treating the name as a command.
    pub fn resolve(&self, name: &str) -> AgentType {
        if let Some(agent_type) = self.types.get(name) {
            agent_type.clone()
        } else {
            // Unknown name — treat as a raw command with no MCP
            AgentType {
                command: name.to_string(),
                args: vec![],
                mcp: false,
                mcp_config_flag: None,
                prompt_flag: None,
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteConfig {
    /// Template for SSH connection. `{name}` is replaced with the host identifier.
    pub connect_command: String,
    /// Available remote hosts.
    pub hosts: Vec<String>,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        Self {
            connect_command: "ssh -t {name}".to_string(),
            hosts: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    pub auto_approve: Vec<String>,
    pub require_approval: Vec<String>,
    pub deny: Vec<String>,
}

impl Default for PermissionsConfig {
    fn default() -> Self {
        Self {
            auto_approve: vec!["file_read".into(), "file_write".into()],
            require_approval: vec!["git_push".into()],
            deny: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub default_layout: LayoutMode,
    pub file_tree_width: u16,
    pub coordinator_width: u16,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            default_layout: LayoutMode::Tiled,
            file_tree_width: 25,
            coordinator_width: 80,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    Tiled,
    Stacked,
    Focused,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub persist: bool,
    pub scrollback_lines: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            persist: true,
            scrollback_lines: 10_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LockConfig {
    pub idle_timeout_secs: u64,
}

impl Default for LockConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NatsConfig {
    pub host: String,
    pub port: u16,
}

impl Default for NatsConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 0,
        }
    }
}

impl ForgeConfig {
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::default_path();
        if path.exists() {
            let contents = std::fs::read_to_string(&path)?;
            let config: ForgeConfig = toml::from_str(&contents)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn load_from(path: &std::path::Path) -> anyhow::Result<Self> {
        let contents = std::fs::read_to_string(path)?;
        let config: ForgeConfig = toml::from_str(&contents)?;
        Ok(config)
    }

    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("", "", "forge")
            .map(|dirs| dirs.config_dir().join("config.toml"))
            .unwrap_or_else(|| PathBuf::from("~/.config/forge/config.toml"))
    }

    pub fn data_dir() -> PathBuf {
        directories::ProjectDirs::from("", "", "forge")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("~/.local/share/forge"))
    }
}
