# Embedding in an AI Agent

## Goal

Use rust-bash as a bash execution tool for LLM-powered agents. The shell provides a sandboxed environment where the AI can run commands, inspect files, and process data — without containers, VMs, or host filesystem access.

## Why rust-bash for AI Agents?

| Feature | rust-bash | Docker/VM | Host bash |
|---------|-----------|-----------|-----------|
| Startup time | Microseconds | Seconds | Microseconds |
| Isolation | Virtual FS, execution limits | Full OS-level | None |
| Memory footprint | KBs | MBs–GBs | N/A |
| Custom commands | VirtualCommand trait | Mount scripts | PATH |
| Network control | URL allow-list | Network policies | iptables |
| Reproducible FS | Yes (InMemoryFs) | Mostly | No |

## TypeScript: Framework-Agnostic Tool Primitives

`@shantanugoel/rust-bash` exports a JSON Schema tool definition and a handler factory that work with **any** AI agent framework — no framework dependencies.

```typescript
import { bashToolDefinition, createBashToolHandler, createNativeBackend } from '@shantanugoel/rust-bash';

// bashToolDefinition is a plain JSON Schema object:
// {
//   name: 'bash',
//   description: 'Execute bash commands in a sandboxed environment...',
//   inputSchema: {
//     type: 'object',
//     properties: { command: { type: 'string', description: '...' } },
//     required: ['command'],
//   },
// }

// createBashToolHandler returns a framework-agnostic handler:
const { handler, definition, bash } = createBashToolHandler(createNativeBackend, {
  files: { '/data.txt': 'hello world' },
  maxOutputLength: 10000,
});

// handler: (args: { command: string }) => Promise<{ stdout, stderr, exitCode }>
const result = await handler({ command: 'grep hello /data.txt' });
```

### Convenience Schema Formatters

Format tool definitions for specific providers without any external dependencies:

```typescript
import { bashToolDefinition, formatToolForProvider } from '@shantanugoel/rust-bash';

const openaiTool = formatToolForProvider(bashToolDefinition, 'openai');
// { type: "function", function: { name: "bash", description: "...", parameters: {...} } }

const anthropicTool = formatToolForProvider(bashToolDefinition, 'anthropic');
// { name: "bash", description: "...", input_schema: {...} }

const mcpTool = formatToolForProvider(bashToolDefinition, 'mcp');
// { name: "bash", description: "...", inputSchema: {...} }
```

### handleToolCall Dispatcher

For agent loops that need to dispatch multiple tool types:

```typescript
import { Bash, handleToolCall } from '@shantanugoel/rust-bash';

// In your agent loop:
const result = await handleToolCall(bash, toolCall.name, toolCall.arguments);
// Supports: 'bash', 'readFile', 'writeFile', 'listDirectory'
```

## Recipe: OpenAI API

```typescript
import OpenAI from 'openai';
import { createBashToolHandler, formatToolForProvider, bashToolDefinition, createNativeBackend } from '@shantanugoel/rust-bash';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });
const openai = new OpenAI();

const response = await openai.chat.completions.create({
  model: 'gpt-4o',
  tools: [formatToolForProvider(bashToolDefinition, 'openai')],
  messages: [{ role: 'user', content: 'List files in /data' }],
});

// In tool call dispatch:
for (const toolCall of response.choices[0].message.tool_calls ?? []) {
  const args = JSON.parse(toolCall.function.arguments);
  const result = await handler(args);
  // Send result back as tool_call response...
}
```

## Recipe: Anthropic API

```typescript
import Anthropic from '@anthropic-ai/sdk';
import { createBashToolHandler, formatToolForProvider, bashToolDefinition, createNativeBackend } from '@shantanugoel/rust-bash';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });
const anthropic = new Anthropic();

const response = await anthropic.messages.create({
  model: 'claude-sonnet-4-20250514',
  max_tokens: 1024,
  tools: [formatToolForProvider(bashToolDefinition, 'anthropic')],
  messages: [{ role: 'user', content: 'List files in /data' }],
});

// In tool call dispatch:
for (const block of response.content) {
  if (block.type === 'tool_use') {
    const result = await handler(block.input);
    // Send result back as tool_result...
  }
}
```

## Recipe: Vercel AI SDK (~8 lines)

```typescript
import { tool } from 'ai';
import { z } from 'zod';
import { createBashToolHandler, createNativeBackend } from '@shantanugoel/rust-bash';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });
const bashTool = tool({
  description: 'Execute bash commands in a sandbox',
  parameters: z.object({ command: z.string() }),
  execute: async ({ command }) => handler({ command }),
});
```

## Recipe: LangChain.js (~8 lines)

```typescript
import { tool } from '@langchain/core/tools';
import { z } from 'zod';
import { createBashToolHandler, createNativeBackend } from '@shantanugoel/rust-bash';

const { handler, definition } = createBashToolHandler(createNativeBackend, { files: myFiles });
const bashTool = tool(
  async ({ command }) => JSON.stringify(await handler({ command })),
  { name: definition.name, description: definition.description, schema: z.object({ command: z.string() }) },
);
```

## Rust: Basic Agent Setup

```rust
use rust_bash::{RustBashBuilder, RustBashError, ExecutionLimits, NetworkPolicy};
use std::collections::HashMap;
use std::time::Duration;

struct AgentShell {
    shell: rust_bash::RustBash,
}

impl AgentShell {
    fn new() -> Self {
        let shell = RustBashBuilder::new()
            .env(HashMap::from([
                ("HOME".into(), "/home/agent".into()),
                ("USER".into(), "agent".into()),
            ]))
            .cwd("/home/agent")
            .execution_limits(ExecutionLimits {
                max_command_count: 5_000,
                max_execution_time: Duration::from_secs(10),
                max_output_size: 512 * 1024, // 512 KB
                ..Default::default()
            })
            .build()
            .unwrap();

        Self { shell }
    }

    /// Execute a command and return a structured result for the LLM.
    fn run(&mut self, command: &str) -> AgentResult {
        match self.shell.exec(command) {
            Ok(result) => AgentResult {
                success: result.exit_code == 0,
                stdout: truncate(&result.stdout, 4096),
                stderr: truncate(&result.stderr, 1024),
                exit_code: result.exit_code,
                error: None,
            },
            Err(RustBashError::LimitExceeded { limit_name, .. }) => AgentResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: -1,
                error: Some(format!("Resource limit exceeded: {limit_name}")),
            },
            Err(e) => AgentResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: -1,
                error: Some(format!("{e}")),
            },
        }
    }
}

struct AgentResult {
    success: bool,
    stdout: String,
    stderr: String,
    exit_code: i32,
    error: Option<String>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let end = s.char_indices()
            .take_while(|(i, _)| *i < max)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}... [truncated, {} total bytes]", &s[..end], s.len())
    }
}
```

## Rust: Tool Definition for Function Calling

```json
{
  "name": "bash",
  "description": "Execute a bash command in a sandboxed environment. The environment has a virtual filesystem, 80+ Unix commands (grep, sed, awk, jq, find, curl, etc.), and full bash syntax (variables, loops, functions, pipes, redirections). State persists between calls.",
  "parameters": {
    "type": "object",
    "properties": {
      "command": {
        "type": "string",
        "description": "The bash command to execute"
      }
    },
    "required": ["command"]
  }
}
```

## MCP Server

For MCP-compatible clients (Claude Desktop, Cursor, VS Code), use the built-in MCP server mode:

```bash
rust-bash --mcp
```

See [MCP Server Setup](mcp-server.md) for configuration details.

## Protecting Against Malicious Scripts

The combination of execution limits, network policy, and InMemoryFs provides defense in depth:

1. **No host filesystem access** — InMemoryFs by default
2. **No network access** — disabled by default; requires explicit allow-list
3. **Resource bounds** — time, commands, output size all capped
4. **No process spawning** — all commands run in-process; no `std::process::Command`
5. **Structured errors** — `LimitExceeded` reports exactly which limit was hit

See [Execution Limits](execution-limits.md) for detailed configuration.
