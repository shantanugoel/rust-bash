# MCP Server Setup

rust-bash includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) (MCP) server, making it instantly available to any MCP-compatible client — Claude Desktop, Cursor, VS Code, Windsurf, Cline, and the OpenAI Agents SDK.

## Quick Start

```bash
rust-bash --mcp
```

This starts an MCP server over stdio using JSON-RPC. The server creates a sandboxed shell instance and maintains state across all tool calls within the session.

## Available Tools

| Tool | Description | Arguments |
|------|-------------|-----------|
| `bash` | Execute bash commands | `{ command: string }` |
| `write_file` | Write content to a file | `{ path: string, content: string }` |
| `read_file` | Read a file's contents | `{ path: string }` |
| `list_directory` | List directory contents | `{ path: string }` |

All file operations are isolated within the in-memory virtual filesystem — no host filesystem access.

## Claude Desktop

Add to your Claude Desktop configuration file:

- **macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Windows**: `%APPDATA%\Claude\claude_desktop_config.json`

```json
{
  "mcpServers": {
    "rust-bash": {
      "command": "rust-bash",
      "args": ["--mcp"]
    }
  }
}
```

## Cursor

Add to your Cursor MCP configuration (`.cursor/mcp.json` in your project or global config):

```json
{
  "mcpServers": {
    "rust-bash": {
      "command": "rust-bash",
      "args": ["--mcp"]
    }
  }
}
```

## VS Code (GitHub Copilot)

Add to `.vscode/mcp.json` in your project:

```json
{
  "servers": {
    "rust-bash": {
      "type": "stdio",
      "command": "rust-bash",
      "args": ["--mcp"]
    }
  }
}
```

Or add to your VS Code settings (`settings.json`):

```json
{
  "mcp": {
    "servers": {
      "rust-bash": {
        "type": "stdio",
        "command": "rust-bash",
        "args": ["--mcp"]
      }
    }
  }
}
```

## Using a Local Build

If you've built rust-bash from source, point to the binary:

```json
{
  "mcpServers": {
    "rust-bash": {
      "command": "/path/to/rust-bash/target/release/rust-bash",
      "args": ["--mcp"]
    }
  }
}
```

## Protocol Details

The MCP server implements the minimal MCP subset over stdio:

- **Transport**: Newline-delimited JSON-RPC over stdin/stdout
- **Methods**: `initialize`, `tools/list`, `tools/call`
- **Notifications**: `notifications/initialized` (acknowledged silently)

### Example Session

```
→ {"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
← {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"rust-bash","version":"0.1.0"}}}

→ {"jsonrpc":"2.0","method":"notifications/initialized"}

→ {"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
← {"jsonrpc":"2.0","id":2,"result":{"tools":[...]}}

→ {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"bash","arguments":{"command":"echo hello"}}}
← {"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"stdout:\nhello\n\nstderr:\n\nexit_code: 0"}]}}
```

## Stateful Sessions

The MCP server maintains a single shell instance across all tool calls. This means:

- Variables set in one `bash` call persist to the next
- Files written via `write_file` or bash redirections are readable in subsequent calls
- The working directory changes from `cd` persist

This is by design — it allows AI agents to build up state across multiple interactions, just like a real shell session.
