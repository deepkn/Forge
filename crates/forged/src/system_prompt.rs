//! System prompt injection — tells MCP-capable agents they're running inside forge.

use anyhow::Result;
use std::path::Path;

/// The forge system instructions written into the agent's working directory.
const FORGE_INSTRUCTIONS: &str = r#"# Forge Environment

You are running inside **Forge**, a terminal IDE for multi-agent coding. You have access to forge MCP tools that let you orchestrate work across multiple agents.

## Available Forge Tools

### Spawning subagents
Use `forge_spawn_agent` to create new agents that work in parallel:
- `task` (required): The task/prompt for the new agent
- `label`: Short name shown in the dashboard (e.g., "auth-impl", "tests")
- `agent_type`: Agent type — "claude", "codex", "shell", etc. Defaults to your same type.
- `working_dir`: Working directory. Defaults to your current directory.

Example: To parallelize work, spawn subagents for independent tasks:
```
forge_spawn_agent(task="Implement the auth module in src/auth/", label="auth-impl")
forge_spawn_agent(task="Write tests for the user API", label="api-tests", agent_type="codex")
```

### Monitoring subagents
- `forge_list_agents()` — See all agents and their status
- `forge_read_output(agent_id, last_n_lines)` — Read an agent's recent terminal output
- `forge_send_message(agent_id, message)` — Send instructions to another agent

### Reporting status
- `forge_report_status(status, message)` — Update your status in the dashboard ("working", "blocked", "reviewing", "done")
- `forge_notify_done(summary, files_changed)` — Signal that your task is complete
- `forge_send_to_coordinator(message)` — Send a message to the coordinator

### File coordination
- `forge_lock_file(path, mode)` — Lock a file ("read" or "write") before editing
- `forge_unlock_file(path)` — Release a file lock when done

## Guidelines
- **Spawn subagents for parallelizable work** rather than doing everything sequentially.
- **Use forge_notify_done** when you finish a task so the coordinator knows.
- **Lock files before writing** when other agents might be working on the same files.
- **Check forge_list_agents** periodically to monitor subagent progress.
"#;

const FORGE_INSTRUCTIONS_FILE: &str = ".forge-instructions.md";

/// Write forge instructions to the working directory so the agent discovers them.
/// For Claude Code, this goes into a CLAUDE.md-compatible location.
pub fn inject_system_prompt(workdir: &Path) -> Result<()> {
    // Write as .forge-instructions.md
    let instructions_path = workdir.join(FORGE_INSTRUCTIONS_FILE);
    std::fs::write(&instructions_path, FORGE_INSTRUCTIONS)?;

    // Also append to CLAUDE.md if it exists (or create it) for Claude Code discovery
    let claude_md_path = workdir.join("CLAUDE.md");
    if claude_md_path.exists() {
        let existing = std::fs::read_to_string(&claude_md_path)?;
        if !existing.contains("Forge Environment") {
            let updated = format!("{}\n\n{}", existing, FORGE_INSTRUCTIONS);
            std::fs::write(&claude_md_path, updated)?;
        }
    } else {
        std::fs::write(&claude_md_path, FORGE_INSTRUCTIONS)?;
    }

    Ok(())
}

/// Clean up forge instructions from the working directory.
pub fn cleanup_system_prompt(workdir: &Path) {
    let instructions_path = workdir.join(FORGE_INSTRUCTIONS_FILE);
    let _ = std::fs::remove_file(instructions_path);

    // Remove forge section from CLAUDE.md if we created the whole file
    // (don't modify if it had pre-existing content)
    let claude_md_path = workdir.join("CLAUDE.md");
    if let Ok(content) = std::fs::read_to_string(&claude_md_path) {
        if content.starts_with("# Forge Environment") && !content.contains("\n# ") {
            // We created this file — safe to remove
            let _ = std::fs::remove_file(claude_md_path);
        }
    }
}
