# Forge

Forge is a terminal-native IDE for multi-agent agentic coding — think "vim/nvim equivalent for AI-assisted development." It allows a developer to orchestrate multiple AI coding agents (Claude Code, Codex, aider, etc.) across local and remote machines from a single keyboard-driven TUI.

Forge is an attempt to fill a gap in modern AI-assisted coding. Currently, AI-assisted coding relies on agent CLIs running in terminals, but there's no purpose-built terminal environment for orchestrating multiple agents simultaneously. 

## Features

- TUI shell for managing agents and sessions
- MCP-based agent communication
- File locking system to prevent conflicts
- PTY-based terminal emulation for agents
- Local agent spawning and management
- NATS-based message bus for inter-agent communication - which can be extended

## Architecture

Forge is built on a client-server architecture with a central daemon and a TUI client.

- **Daemon**: `forged` - runs in the background and manages agents and sessions
- **Client**: `forge` - TUI client for interacting with the daemon
- **Message Bus**: `nats` - message bus for inter-agent communication
- **State Storage**: `sqlite` - database for storing agent and session information
- **MCP Bridge**: `forge-mcp` - MCP bridge for inter-agent communication - advertises forge's capabilities as MCP tools to the agent and handles the MCP <-> NATS translation for inter agent communication.

## Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/forge.git
cd forge

# Build the project
cargo build --release
```

## Usage

### Starting the Daemon

```bash
./target/release/forged
```

### Starting the TUI

```bash
./target/release/forge
```

### Spawning an Agent

From the TUI, press `Esc` then `n` to spawn a new agent.

### Stopping the Daemon

```bash
./target/release/forge stop
```

## Configuration

Configuration is managed through `forge.toml` in the data directory.

## License

[MIT License](LICENSE)