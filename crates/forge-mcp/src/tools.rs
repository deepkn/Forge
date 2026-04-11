//! MCP tool definitions and handlers.
//!
//! Coordinator gets: spawn_agent, list_agents, read_output, send_message, kill_agent, check_inbox
//! All agents get: report_status, notify_done, send_to_coordinator, lock_file, unlock_file
//! Subagents additionally get: spawn_agent (to create siblings)

use anyhow::{Context, Result};
use forge_core::protocol::{
    CoordinatorCommand, CoordinatorNotification, McpToolCall, NotificationType,
};
use forge_core::subjects::{AgentSubjects, CoordinatorInbox, CoordinatorSubjects, SessionSubjects};
use forge_core::types::{AgentHost, AgentId, LockMode};
use serde_json::Value;

/// Return MCP tool definitions based on the agent's role.
pub fn tool_definitions(is_coordinator: bool) -> Vec<Value> {
    let mut tools = vec![
        // ─── Available to all agents ─────────────────────────────
        tool_def(
            "forge_report_status",
            "Report your current status to the forge dashboard and coordinator.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "status": {
                        "type": "string",
                        "enum": ["working", "blocked", "reviewing", "done"],
                        "description": "Current status"
                    },
                    "message": {
                        "type": "string",
                        "description": "Status message"
                    }
                },
                "required": ["status", "message"]
            }),
        ),
        tool_def(
            "forge_notify_done",
            "Signal that your assigned task is complete. Provides a summary to the coordinator.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "Summary of completed work"
                    },
                    "files_changed": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "List of files modified"
                    }
                },
                "required": ["summary"]
            }),
        ),
        tool_def(
            "forge_send_to_coordinator",
            "Send a message to the coordinator agent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Message content"
                    }
                },
                "required": ["message"]
            }),
        ),
        tool_def(
            "forge_lock_file",
            "Acquire a read or write lock on a file to coordinate with other agents.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to lock" },
                    "mode": { "type": "string", "enum": ["read", "write"], "description": "Lock mode" }
                },
                "required": ["path", "mode"]
            }),
        ),
        tool_def(
            "forge_unlock_file",
            "Release a file lock.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "File path to unlock" }
                },
                "required": ["path"]
            }),
        ),
        // ─── Agent management (available to coordinator and subagents) ───
        tool_def(
            "forge_spawn_agent",
            "Spawn a new agent to work on a task. The agent runs in a separate terminal pane.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "description": "The task/prompt to assign to the new agent"
                    },
                    "label": {
                        "type": "string",
                        "description": "Short display name for the agent (shown in dock)"
                    },
                    "agent_type": {
                        "type": "string",
                        "description": "Agent type: 'claude', 'codex', 'shell', etc. Defaults to config."
                    },
                    "working_dir": {
                        "type": "string",
                        "description": "Working directory for the agent. Defaults to current."
                    }
                },
                "required": ["task"]
            }),
        ),
        tool_def(
            "forge_list_agents",
            "List all running agents with their current status.",
            serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        ),
        tool_def(
            "forge_read_output",
            "Read recent terminal output from another agent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "description": "Agent ID to read from" },
                    "last_n_lines": { "type": "integer", "description": "Number of lines (default 50)" }
                },
                "required": ["agent_id"]
            }),
        ),
        tool_def(
            "forge_send_message",
            "Send a text message to another agent (typed into their terminal).",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "description": "Target agent ID" },
                    "message": { "type": "string", "description": "Message to send" }
                },
                "required": ["agent_id", "message"]
            }),
        ),
    ];

    if is_coordinator {
        tools.push(tool_def(
            "forge_kill_agent",
            "Kill a running agent.",
            serde_json::json!({
                "type": "object",
                "properties": {
                    "agent_id": { "type": "string", "description": "Agent ID to kill" }
                },
                "required": ["agent_id"]
            }),
        ));
        tools.push(tool_def(
            "forge_check_inbox",
            "Check for messages and notifications from subagents.",
            serde_json::json!({
                "type": "object",
                "properties": {}
            }),
        ));
    }

    tools
}

fn tool_def(name: &str, description: &str, input_schema: Value) -> Value {
    serde_json::json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema
    })
}

/// Handle an MCP tool call by publishing to NATS and returning the result.
pub async fn handle_tool_call(
    tool_name: &str,
    arguments: &Value,
    agent_id: &str,
    is_coordinator: bool,
    nats: &async_nats::Client,
) -> Result<Value> {
    match tool_name {
        "forge_spawn_agent" => {
            let task = arguments
                .get("task")
                .and_then(|v| v.as_str())
                .context("task is required")?
                .to_string();
            let label = arguments
                .get("label")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("agent-{}", &uuid::Uuid::new_v4().to_string()[..4]));
            let agent_type = arguments
                .get("agent_type")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let working_dir = arguments
                .get("working_dir")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| {
                    std::env::current_dir()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string()
                });

            let cmd = CoordinatorCommand::SpawnAgent {
                label: label.clone(),
                agent_type,
                command: None,
                args: None,
                host: AgentHost::Local,
                working_dir,
                task: Some(task),
            };
            let payload = serde_json::to_vec(&cmd)?;
            nats.publish(CoordinatorSubjects::command(), payload.into())
                .await?;

            Ok(serde_json::json!({
                "status": "spawning",
                "label": label,
                "message": "Agent is being created. Use forge_list_agents to check its status."
            }))
        }

        "forge_list_agents" => {
            // Request session snapshot to get agent list
            let reply = nats
                .request(
                    SessionSubjects::state(),
                    bytes::Bytes::new(),
                )
                .await
                .context("Failed to get agent list")?;

            let snapshot: forge_core::protocol::SessionSnapshot =
                serde_json::from_slice(&reply.payload)?;

            let agents: Vec<Value> = snapshot
                .agents
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "agent_id": a.id.as_str(),
                        "label": a.label,
                        "state": format!("{}", a.state),
                        "host": format!("{}", a.host),
                        "agent_type": a.agent_type,
                        "mcp_capable": a.mcp_capable,
                        "working_dir": a.working_dir,
                    })
                })
                .collect();

            Ok(serde_json::json!({ "agents": agents }))
        }

        "forge_read_output" => {
            let target_id = arguments
                .get("agent_id")
                .and_then(|v| v.as_str())
                .context("agent_id is required")?;
            let n = arguments
                .get("last_n_lines")
                .and_then(|v| v.as_u64())
                .unwrap_or(50) as usize;

            let cmd = CoordinatorCommand::ReadOutput {
                agent_id: AgentId(target_id.to_string()),
                last_n_lines: n,
            };
            let payload = serde_json::to_vec(&cmd)?;
            let reply = nats
                .request(CoordinatorSubjects::command(), payload.into())
                .await
                .context("Failed to read output")?;

            let result: Value = serde_json::from_slice(&reply.payload).unwrap_or(serde_json::json!({
                "lines": [],
                "error": "No response from daemon"
            }));
            Ok(result)
        }

        "forge_send_message" => {
            let target_id = arguments
                .get("agent_id")
                .and_then(|v| v.as_str())
                .context("agent_id is required")?;
            let message = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .context("message is required")?;

            let cmd = CoordinatorCommand::SendMessage {
                agent_id: AgentId(target_id.to_string()),
                message: message.to_string(),
            };
            let payload = serde_json::to_vec(&cmd)?;
            nats.publish(CoordinatorSubjects::command(), payload.into())
                .await?;

            Ok(serde_json::json!({ "sent": true }))
        }

        "forge_kill_agent" => {
            if !is_coordinator {
                anyhow::bail!("Only the coordinator can kill agents");
            }
            let target_id = arguments
                .get("agent_id")
                .and_then(|v| v.as_str())
                .context("agent_id is required")?;

            let cmd = CoordinatorCommand::KillAgent {
                agent_id: AgentId(target_id.to_string()),
            };
            let payload = serde_json::to_vec(&cmd)?;
            nats.publish(CoordinatorSubjects::command(), payload.into())
                .await?;

            Ok(serde_json::json!({ "killed": true }))
        }

        "forge_report_status" => {
            let status = arguments
                .get("status")
                .and_then(|v| v.as_str())
                .context("status is required")?;
            let message = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .context("message is required")?;

            let state = match status {
                "done" => forge_core::types::AgentState::Done,
                "blocked" => forge_core::types::AgentState::Waiting,
                _ => forge_core::types::AgentState::Running,
            };

            let event = forge_core::protocol::AgentStateEvent {
                agent_id: AgentId(agent_id.to_string()),
                state,
                message: Some(message.to_string()),
            };
            let payload = serde_json::to_vec(&event)?;
            nats.publish(
                AgentSubjects::state(&AgentId(agent_id.to_string())),
                payload.into(),
            )
            .await?;

            Ok(serde_json::json!({ "ack": true }))
        }

        "forge_notify_done" => {
            let summary = arguments
                .get("summary")
                .and_then(|v| v.as_str())
                .context("summary is required")?;
            let files: Vec<String> = arguments
                .get("files_changed")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();

            // Update agent state to Done
            let event = forge_core::protocol::AgentStateEvent {
                agent_id: AgentId(agent_id.to_string()),
                state: forge_core::types::AgentState::Done,
                message: Some(summary.to_string()),
            };
            let payload = serde_json::to_vec(&event)?;
            nats.publish(
                AgentSubjects::state(&AgentId(agent_id.to_string())),
                payload.into(),
            )
            .await?;

            // Notify coordinator
            let notification = CoordinatorNotification {
                from_agent_id: AgentId(agent_id.to_string()),
                from_label: agent_id.to_string(),
                notification_type: NotificationType::Done,
                content: format!(
                    "Task completed.\nSummary: {}\nFiles: {}",
                    summary,
                    if files.is_empty() {
                        "(none)".to_string()
                    } else {
                        files.join(", ")
                    }
                ),
            };
            let payload = serde_json::to_vec(&notification)?;
            nats.publish(CoordinatorInbox::notifications(), payload.into())
                .await?;

            Ok(serde_json::json!({ "ack": true }))
        }

        "forge_send_to_coordinator" => {
            let message = arguments
                .get("message")
                .and_then(|v| v.as_str())
                .context("message is required")?;

            let notification = CoordinatorNotification {
                from_agent_id: AgentId(agent_id.to_string()),
                from_label: agent_id.to_string(),
                notification_type: NotificationType::Message,
                content: message.to_string(),
            };
            let payload = serde_json::to_vec(&notification)?;
            nats.publish(CoordinatorInbox::notifications(), payload.into())
                .await?;

            Ok(serde_json::json!({ "delivered": true }))
        }

        "forge_lock_file" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .context("path is required")?;
            let mode_str = arguments
                .get("mode")
                .and_then(|v| v.as_str())
                .context("mode is required")?;
            let mode = match mode_str {
                "write" => LockMode::Write,
                _ => LockMode::Read,
            };

            let request = forge_core::protocol::LockRequest::Acquire {
                agent_id: AgentId(agent_id.to_string()),
                path: path.to_string(),
                mode,
            };
            let payload = serde_json::to_vec(&request)?;
            let reply = nats
                .request(
                    forge_core::subjects::FileLockSubjects::request(),
                    payload.into(),
                )
                .await
                .context("Lock request failed")?;

            let response: Value = serde_json::from_slice(&reply.payload)?;
            Ok(response)
        }

        "forge_unlock_file" => {
            let path = arguments
                .get("path")
                .and_then(|v| v.as_str())
                .context("path is required")?;

            let request = forge_core::protocol::LockRequest::Release {
                agent_id: AgentId(agent_id.to_string()),
                path: path.to_string(),
            };
            let payload = serde_json::to_vec(&request)?;
            nats.publish(
                forge_core::subjects::FileLockSubjects::request(),
                payload.into(),
            )
            .await?;

            Ok(serde_json::json!({ "released": true }))
        }

        "forge_check_inbox" => {
            if !is_coordinator {
                anyhow::bail!("Only the coordinator can check the inbox");
            }
            // TODO: drain queued notifications from NATS JetStream or a buffer
            // For now, return empty (notifications are injected into PTY stdin)
            Ok(serde_json::json!({ "messages": [] }))
        }

        _ => anyhow::bail!("Unknown tool: {tool_name}"),
    }
}
