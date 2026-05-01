//! State file for daemon ↔ TUI coordination.
//!
//! forged writes a state file on startup containing its NATS URL and PID.
//! forge reads this file to discover and connect to the daemon.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    pub pid: u32,
    pub nats_url: String,
    pub workdir: String,
    /// Session ID for re-attach.
    #[serde(default)]
    pub session_id: String,
}

impl DaemonState {
    /// Write the state file.
    pub fn write(&self) -> Result<()> {
        let path = state_file_path();
        std::fs::create_dir_all(path.parent().unwrap())?;
        let contents = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, contents).context("Failed to write daemon state file")?;
        Ok(())
    }

    /// Read the state file. Returns None if it doesn't exist.
    pub fn read() -> Result<Option<Self>> {
        let path = state_file_path();
        if !path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&path)?;
        let state: DaemonState = serde_json::from_str(&contents)?;
        Ok(Some(state))
    }

    /// Remove the state file (on daemon shutdown).
    pub fn remove() {
        let _ = std::fs::remove_file(state_file_path());
    }

    /// Check if the daemon process is still alive.
    pub fn is_alive(&self) -> bool {
        // Check if process exists via kill(pid, 0)
        unsafe { libc::kill(self.pid as i32, 0) == 0 }
    }
}

fn state_file_path() -> PathBuf {
    crate::config::ForgeConfig::data_dir().join("daemon.json")
}
