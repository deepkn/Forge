//! forge-mcp — MCP server that bridges agent CLIs to forged via NATS.
//!
//! Communicates with the agent CLI over stdio (MCP JSON-RPC protocol),
//! and with forged over NATS. Each agent instance gets its own forge-mcp process.
//!
//! Environment variables:
//!   FORGE_NATS_URL   — NATS connection URL (required)
//!   FORGE_AGENT_ID   — this agent's ID (required)
//!   FORGE_AGENT_ROLE — "coordinator" or "subagent" (default: "subagent")

mod tools;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};

/// MCP JSON-RPC request.
#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// MCP JSON-RPC response.
#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

fn main() -> Result<()> {
    let nats_url = std::env::var("FORGE_NATS_URL")
        .context("FORGE_NATS_URL environment variable is required")?;
    let agent_id = std::env::var("FORGE_AGENT_ID")
        .context("FORGE_AGENT_ID environment variable is required")?;
    let agent_role = std::env::var("FORGE_AGENT_ROLE").unwrap_or_else(|_| "subagent".to_string());
    let is_coordinator = agent_role == "coordinator";

    // Build the tokio runtime for async NATS operations
    let rt = tokio::runtime::Runtime::new()?;

    let nats = rt
        .block_on(async_nats::connect(&nats_url))
        .context("Failed to connect to NATS")?;

    let tool_defs = tools::tool_definitions(is_coordinator);

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_lock = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };

        if line.trim().is_empty() {
            continue;
        }

        let request: JsonRpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("forge-mcp: invalid JSON-RPC: {e}");
                continue;
            }
        };

        let response = match request.method.as_str() {
            "initialize" => {
                let result = serde_json::json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "forge",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                });
                make_response(request.id, Some(result), None)
            }
            "notifications/initialized" => {
                // Client ack — no response needed for notifications
                continue;
            }
            "tools/list" => {
                let result = serde_json::json!({ "tools": tool_defs });
                make_response(request.id, Some(result), None)
            }
            "tools/call" => {
                let tool_name = request.params.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let arguments = request.params.get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::json!({}));

                let result = rt.block_on(tools::handle_tool_call(
                    tool_name,
                    &arguments,
                    &agent_id,
                    is_coordinator,
                    &nats,
                ));

                match result {
                    Ok(value) => {
                        let content = serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": serde_json::to_string_pretty(&value).unwrap_or_default()
                            }]
                        });
                        make_response(request.id, Some(content), None)
                    }
                    Err(e) => {
                        let content = serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": format!("Error: {e}")
                            }],
                            "isError": true
                        });
                        make_response(request.id, Some(content), None)
                    }
                }
            }
            _ => make_response(
                request.id,
                None,
                Some(JsonRpcError {
                    code: -32601,
                    message: format!("Method not found: {}", request.method),
                }),
            ),
        };

        let json = serde_json::to_string(&response).unwrap();
        let _ = writeln!(stdout_lock, "{json}");
        let _ = stdout_lock.flush();
    }

    Ok(())
}

fn make_response(
    id: Option<serde_json::Value>,
    result: Option<serde_json::Value>,
    error: Option<JsonRpcError>,
) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id: id.unwrap_or(serde_json::Value::Null),
        result,
        error,
    }
}
