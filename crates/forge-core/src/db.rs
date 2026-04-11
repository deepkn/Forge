//! SQLite database layer for Forge state persistence.

use crate::types::{AgentColor, AgentHost, AgentId, AgentInfo, AgentState, LockMode, SessionId};
use anyhow::Result;
use rusqlite::{params, Connection};
use std::path::Path;

/// Initialize the database, creating tables if they don't exist.
pub fn init(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    create_tables(&conn)?;
    Ok(conn)
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS agents (
            id TEXT PRIMARY KEY,
            label TEXT NOT NULL,
            command TEXT NOT NULL,
            args TEXT NOT NULL DEFAULT '[]',
            host_type TEXT NOT NULL DEFAULT 'local',
            host_name TEXT,
            working_dir TEXT NOT NULL,
            state TEXT NOT NULL DEFAULT 'starting',
            color TEXT NOT NULL DEFAULT 'blue',
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            finished_at TEXT
        );

        CREATE TABLE IF NOT EXISTS file_locks (
            path TEXT NOT NULL,
            agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            mode TEXT NOT NULL,
            acquired_at TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (path, agent_id)
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            layout TEXT,
            created_at TEXT NOT NULL DEFAULT (datetime('now')),
            last_connected TEXT
        );

        CREATE TABLE IF NOT EXISTS agent_assignments (
            agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
            session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
            pane_id TEXT,
            PRIMARY KEY (agent_id, session_id)
        );
        ",
    )?;
    Ok(())
}

// ─── Agent operations ───────────────────────────────────────────────

pub fn insert_agent(conn: &Connection, agent: &AgentInfo) -> Result<()> {
    let (host_type, host_name) = match &agent.host {
        AgentHost::Local => ("local", None),
        AgentHost::Remote { name } => ("remote", Some(name.as_str())),
    };
    let args_json = serde_json::to_string(&agent.args)?;
    let color = serde_json::to_string(&agent.color)?;
    conn.execute(
        "INSERT INTO agents (id, label, command, args, host_type, host_name, working_dir, state, color)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            agent.id.as_str(),
            agent.label,
            agent.command,
            args_json,
            host_type,
            host_name,
            agent.working_dir,
            agent.state.to_string(),
            color,
        ],
    )?;
    Ok(())
}

pub fn update_agent_state(conn: &Connection, id: &AgentId, state: &AgentState) -> Result<()> {
    let finished_at = match state {
        AgentState::Done | AgentState::Error => {
            Some(chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string())
        }
        _ => None,
    };
    conn.execute(
        "UPDATE agents SET state = ?1, finished_at = COALESCE(?2, finished_at) WHERE id = ?3",
        params![state.to_string(), finished_at, id.as_str()],
    )?;
    Ok(())
}

pub fn get_all_agents(conn: &Connection) -> Result<Vec<AgentInfo>> {
    let mut stmt =
        conn.prepare("SELECT id, label, command, args, host_type, host_name, working_dir, state, color FROM agents")?;
    let agents = stmt
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let label: String = row.get(1)?;
            let command: String = row.get(2)?;
            let args_json: String = row.get(3)?;
            let host_type: String = row.get(4)?;
            let host_name: Option<String> = row.get(5)?;
            let working_dir: String = row.get(6)?;
            let state_str: String = row.get(7)?;
            let color_str: String = row.get(8)?;
            Ok((
                id,
                label,
                command,
                args_json,
                host_type,
                host_name,
                working_dir,
                state_str,
                color_str,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    agents
        .into_iter()
        .map(
            |(id, label, command, args_json, host_type, host_name, working_dir, state_str, color_str)| {
                let args: Vec<String> = serde_json::from_str(&args_json).unwrap_or_default();
                let host = if host_type == "remote" {
                    AgentHost::Remote {
                        name: host_name.unwrap_or_default(),
                    }
                } else {
                    AgentHost::Local
                };
                let state = serde_json::from_str(&format!("\"{state_str}\""))
                    .unwrap_or(AgentState::Starting);
                let color =
                    serde_json::from_str(&color_str).unwrap_or(AgentColor::Blue);
                Ok(AgentInfo {
                    id: AgentId(id),
                    label,
                    command,
                    args,
                    host,
                    working_dir,
                    state,
                    color,
                    mcp_capable: false,
                    agent_type: String::new(),
                })
            },
        )
        .collect()
}

pub fn delete_agent(conn: &Connection, id: &AgentId) -> Result<()> {
    conn.execute("DELETE FROM agents WHERE id = ?1", params![id.as_str()])?;
    Ok(())
}

// ─── Lock operations ────────────────────────────────────────────────

pub fn acquire_lock(
    conn: &Connection,
    path: &str,
    agent_id: &AgentId,
    mode: &LockMode,
) -> Result<bool> {
    // Check for conflicting locks
    let has_writer: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM file_locks WHERE path = ?1 AND mode = 'write'",
            params![path],
            |row| row.get(0),
        )
        .unwrap_or(false);

    match mode {
        LockMode::Read => {
            if has_writer {
                return Ok(false);
            }
        }
        LockMode::Write => {
            let has_any: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM file_locks WHERE path = ?1",
                    params![path],
                    |row| row.get(0),
                )
                .unwrap_or(false);
            if has_any {
                return Ok(false);
            }
        }
    }

    let mode_str = match mode {
        LockMode::Read => "read",
        LockMode::Write => "write",
    };
    conn.execute(
        "INSERT OR REPLACE INTO file_locks (path, agent_id, mode) VALUES (?1, ?2, ?3)",
        params![path, agent_id.as_str(), mode_str],
    )?;
    Ok(true)
}

pub fn release_lock(conn: &Connection, path: &str, agent_id: &AgentId) -> Result<()> {
    conn.execute(
        "DELETE FROM file_locks WHERE path = ?1 AND agent_id = ?2",
        params![path, agent_id.as_str()],
    )?;
    Ok(())
}

pub fn force_release_lock(conn: &Connection, path: &str) -> Result<()> {
    conn.execute("DELETE FROM file_locks WHERE path = ?1", params![path])?;
    Ok(())
}

pub fn get_all_locks(
    conn: &Connection,
) -> Result<Vec<(String, AgentId, LockMode, String)>> {
    let mut stmt =
        conn.prepare("SELECT path, agent_id, mode, acquired_at FROM file_locks")?;
    let locks = stmt
        .query_map([], |row| {
            let path: String = row.get(0)?;
            let agent_id: String = row.get(1)?;
            let mode: String = row.get(2)?;
            let acquired_at: String = row.get(3)?;
            Ok((path, agent_id, mode, acquired_at))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    locks
        .into_iter()
        .map(|(path, agent_id, mode_str, acquired_at)| {
            let mode = if mode_str == "write" {
                LockMode::Write
            } else {
                LockMode::Read
            };
            Ok((path, AgentId(agent_id), mode, acquired_at))
        })
        .collect()
}

// ─── Session operations ─────────────────────────────────────────────

pub fn create_session(conn: &Connection, id: &SessionId) -> Result<()> {
    conn.execute(
        "INSERT INTO sessions (id) VALUES (?1)",
        params![id.as_str()],
    )?;
    Ok(())
}

pub fn update_session_layout(conn: &Connection, id: &SessionId, layout: &str) -> Result<()> {
    conn.execute(
        "UPDATE sessions SET layout = ?1, last_connected = datetime('now') WHERE id = ?2",
        params![layout, id.as_str()],
    )?;
    Ok(())
}

pub fn get_latest_session(conn: &Connection) -> Result<Option<(SessionId, Option<String>)>> {
    let result = conn.query_row(
        "SELECT id, layout FROM sessions ORDER BY last_connected DESC, created_at DESC LIMIT 1",
        [],
        |row| {
            let id: String = row.get(0)?;
            let layout: Option<String> = row.get(1)?;
            Ok((SessionId(id), layout))
        },
    );
    match result {
        Ok(session) => Ok(Some(session)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
