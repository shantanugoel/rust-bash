# Chapter 8: Integration Targets

## Overview

rust-bash is designed to be embedded anywhere. This chapter covers the integration surfaces: Rust crate API, CLI binary, C FFI, WASM, and AI SDK tool definitions.

## Rust Crate API

The primary interface. All other integration targets are thin wrappers around this.

```rust
use rust_bash::{RustBashBuilder, ExecResult};
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/data.txt".into(), b"hello world".to_vec()),
        ("/config.json".into(), b"{}".to_vec()),
    ]))
    .env(HashMap::from([
        ("USER".into(), "agent".into()),
        ("HOME".into(), "/home/agent".into()),
    ]))
    .cwd("/")
    .build()
    .unwrap();

let result: ExecResult = shell.exec("cat /data.txt | grep hello").unwrap();
assert_eq!(result.stdout, "hello world\n");
assert_eq!(result.exit_code, 0);
```

### RustBashBuilder

```rust
RustBashBuilder::new()
    .files(HashMap<String, Vec<u8>>)     // Seed VFS with files (path → bytes)
    .env(HashMap<String, String>)        // Set environment variables
    .cwd("/path")                        // Set working directory (created automatically)
    .execution_limits(limits)            // Configure limits
    .network_policy(policy)              // Configure network access
    .fs(Arc<dyn VirtualFs>)              // Use a custom filesystem backend
    .command(Box::new(custom_cmd))       // Register a custom command
    .build()                             // Returns Result<RustBash, RustBashError>
```

### ExecResult

```rust
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
```

## CLI Binary

A standalone binary for command-line usage (Milestone M5.1).

```bash
# Execute a command
rust-bash -c 'echo hello | wc -c'

# Execute a script file with positional arguments
rust-bash script.sh arg1 arg2

# Read commands from stdin
echo 'echo hello' | rust-bash

# Seed files from disk
rust-bash --files /data:/app/data.txt --files /config:/app/config.json -c 'cat /app/data.txt'

# Set environment
rust-bash --env USER=agent --env HOME=/home/agent -c 'echo $USER'

# JSON output for machine consumption
rust-bash --json -c 'echo hello'
# {"stdout":"hello\n","stderr":"","exit_code":0}

# Interactive REPL (starts when no command/script/stdin is given)
rust-bash
```

### Interactive REPL

When launched without `-c`, a script file, or piped stdin, `rust-bash` starts an
interactive REPL with readline support:

- **Colored prompt**: `rust-bash:{cwd}$ ` — green after exit 0, red after non-zero
- **Tab completion**: completes built-in command names (first token only)
- **Multi-line input**: incomplete constructs wait for continuation input
- **History**: loaded from / saved to `~/.rust_bash_history`
- **Ctrl-C**: cancels the current input line
- **Ctrl-D**: exits the REPL with the last command's exit code
- **`exit [N]`**: exits with code N (default 0)
- **`--json`**: rejected in REPL mode (exits with code 2)

> An interactive REPL is also available as a runnable example showing library-level embedding:
> `cargo run --example shell`

The CLI binary compiles as a single binary with no additional runtime dependencies beyond libc.

## C FFI

For embedding in Python, Go, Ruby, or any language with C interop.

### Build

```bash
cargo build --features ffi --release
# Output: target/release/librust_bash.so (Linux), .dylib (macOS), .dll (Windows)
# Header: include/rust_bash.h
```

### API

Six functions are exported (see `include/rust_bash.h` for full documentation):

```c
#include "rust_bash.h"

// Lifecycle
struct RustBash *rust_bash_create(const char *config_json); // NULL config → defaults
void             rust_bash_free(struct RustBash *sb);       // NULL-safe no-op

// Execution
struct ExecResult *rust_bash_exec(struct RustBash *sb, const char *command);
void               rust_bash_result_free(struct ExecResult *result); // NULL-safe no-op

// Diagnostics
const char *rust_bash_last_error(void); // NULL if no error; do not free
const char *rust_bash_version(void);    // static string; do not free
```

The `ExecResult` struct:

```c
typedef struct ExecResult {
    const char *stdout_ptr;
    int32_t     stdout_len;
    const char *stderr_ptr;
    int32_t     stderr_len;
    int32_t     exit_code;
} ExecResult;
```

### Configuration via JSON

Config is passed as a JSON string to `rust_bash_create`. All fields are optional — an empty `{}` or `NULL` produces a default-configured sandbox.

```json
{
  "files": {
    "/data.txt": "content",
    "/config.json": "{}"
  },
  "env": {
    "USER": "agent",
    "HOME": "/home/agent"
  },
  "cwd": "/",
  "limits": {
    "max_command_count": 10000,
    "max_execution_time_secs": 30,
    "max_loop_iterations": 10000,
    "max_output_size": 10485760,
    "max_call_depth": 25,
    "max_string_length": 10485760,
    "max_glob_results": 100000,
    "max_substitution_depth": 50,
    "max_heredoc_size": 10485760,
    "max_brace_expansion": 10000
  },
  "network": {
    "enabled": true,
    "allowed_url_prefixes": ["https://api.example.com/"],
    "allowed_methods": ["GET", "POST"],
    "max_response_size": 10485760,
    "max_redirects": 5,
    "timeout_secs": 30
  }
}
```

### Memory Ownership

- `rust_bash_create` returns a heap-allocated sandbox; caller must call `rust_bash_free`.
- `rust_bash_exec` returns a heap-allocated result; caller must call `rust_bash_result_free`.
- String pointers in `ExecResult` are valid until `rust_bash_result_free` is called.
- `rust_bash_version` returns a static string — do not free.
- `rust_bash_last_error` returns a pointer into thread-local storage — valid only until the next FFI call on the same thread; do not free.

### Thread Safety

A `RustBash*` handle must not be shared across threads without external synchronization. Each handle is independently owned; different handles may be used concurrently from different threads. The last-error storage (`rust_bash_last_error`) is thread-local, so error messages are per-thread.

### Error Handling

Functions that can fail (`rust_bash_create`, `rust_bash_exec`) return `NULL` on error. After a `NULL` return, call `rust_bash_last_error()` on the same thread to retrieve a human-readable error message. The error string is valid until the next FFI call on that thread.

```c
struct RustBash *sb = rust_bash_create("{invalid json}");
if (!sb) {
    fprintf(stderr, "Error: %s\n", rust_bash_last_error());
}
```

### Python Example

```python
import ctypes

class ExecResult(ctypes.Structure):
    _fields_ = [
        ("stdout_ptr", ctypes.c_void_p),
        ("stdout_len", ctypes.c_int32),
        ("stderr_ptr", ctypes.c_void_p),
        ("stderr_len", ctypes.c_int32),
        ("exit_code", ctypes.c_int32),
    ]

lib = ctypes.CDLL("./target/release/librust_bash.so")

lib.rust_bash_create.argtypes = [ctypes.c_char_p]
lib.rust_bash_create.restype = ctypes.c_void_p
lib.rust_bash_exec.argtypes = [ctypes.c_void_p, ctypes.c_char_p]
lib.rust_bash_exec.restype = ctypes.POINTER(ExecResult)
lib.rust_bash_result_free.argtypes = [ctypes.POINTER(ExecResult)]
lib.rust_bash_free.argtypes = [ctypes.c_void_p]
lib.rust_bash_last_error.restype = ctypes.c_char_p

sb = lib.rust_bash_create(b'{"files":{"/data.txt":"hello"}}')
if not sb:
    print("Error:", lib.rust_bash_last_error())
else:
    result = lib.rust_bash_exec(sb, b"cat /data.txt")
    if result:
        r = result.contents
        stdout = ctypes.string_at(r.stdout_ptr, r.stdout_len)
        print(stdout)  # b'hello\n'
        print("exit code:", r.exit_code)
        lib.rust_bash_result_free(result)
    lib.rust_bash_free(sb)
```

### Go Example

```go
package main

/*
#cgo LDFLAGS: -L./target/release -lrust_bash
#include "include/rust_bash.h"
#include <stdlib.h>
*/
import "C"
import (
	"fmt"
	"unsafe"
)

func main() {
	sb := C.rust_bash_create(nil)
	if sb == nil {
		panic(C.GoString(C.rust_bash_last_error()))
	}
	defer C.rust_bash_free(sb)

	cmd := C.CString("echo hello world")
	defer C.free(unsafe.Pointer(cmd))

	r := C.rust_bash_exec(sb, cmd)
	if r == nil {
		panic(C.GoString(C.rust_bash_last_error()))
	}
	defer C.rust_bash_result_free(r)

	fmt.Printf("%s", C.GoStringN(r.stdout_ptr, C.int(r.stdout_len)))
	fmt.Printf("exit code: %d\n", r.exit_code)
}
```

## WASM Target

For browser and edge runtime embedding. The Rust interpreter compiles to `wasm32-unknown-unknown` via `wasm-bindgen`.

### Build

```bash
# Using the build script
./scripts/build-wasm.sh

# Or manually
cargo build --target wasm32-unknown-unknown --features wasm --no-default-features --release
wasm-bindgen target/wasm32-unknown-unknown/release/rust_bash.wasm --out-dir pkg --target bundler
```

### Platform Abstraction

- `std::time::{SystemTime, Instant}` → `crate::platform::*` (uses `web-time` crate on WASM)
- `std::thread::sleep` → returns error "sleep: not supported in browser environment"
- `chrono` uses `wasmbind` feature on WASM for timezone support
- `ureq`/`url` (networking) feature-gated behind `network` — disabled on WASM
- `OverlayFs`/`ReadWriteFs` feature-gated behind `native-fs` — WASM only gets `InMemoryFs`/`MountableFs`
- `parking_lot` compiles to WASM (falls back to spin-locks)

### Cargo Features for WASM

```toml
[features]
default = ["cli", "network", "native-fs"]
wasm = ["dep:wasm-bindgen", "dep:js-sys", "dep:serde", "dep:serde-wasm-bindgen"]
network = ["dep:ureq", "dep:url"]    # disabled on WASM
native-fs = []                         # disabled on WASM
```

### Compatibility Notes

- brush-parser compiles to `wasm32-unknown-unknown`
- The interpreter and VFS are pure Rust with no OS dependencies
- `web-time` crate provides `SystemTime`/`Instant` replacements for WASM
- Networking (`curl`) is feature-gated out; returns "command not found" on WASM

## npm Package (`rust-bash`)

The TypeScript npm package wraps both WASM and native addon backends behind a unified API.

### Installation

```bash
npm install rust-bash
```

### Architecture

The package ships three layers:

1. **TypeScript API** (`Bash` class, `defineCommand`, tool primitives) — the public interface
2. **Native addons** (napi-rs) — bundled Linux/macOS x64 and arm64 binaries for Node.js
3. **WASM backend** — browser and edge runtime support

Backend detection is automatic on Node.js (matching bundled native binary first,
WASM fallback). Browsers use the `rust-bash/browser` entry point (WASM only).

### Quick Start (Node.js)

```typescript
import { Bash, tryLoadNative, createNativeBackend, initWasm, createWasmBackend } from 'rust-bash';

// Auto-detect backend
let createBackend;
if (await tryLoadNative()) {
  createBackend = createNativeBackend;
} else {
  await initWasm();
  createBackend = createWasmBackend;
}

const bash = await Bash.create(createBackend, {
  files: { '/data.txt': 'hello world' },
  env: { USER: 'agent' },
});

const result = await bash.exec('cat /data.txt | grep hello');
console.log(result.stdout); // "hello world\n"
```

### Quick Start (Browser)

```typescript
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend, {
  files: { '/hello.txt': 'Hello from WASM!' },
  cwd: '/home/user',
});

const result = await bash.exec('cat /hello.txt');
console.log(result.stdout); // "Hello from WASM!\n"
```

### Bash Class API

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    '/data.txt': 'hello world',              // eager
    '/lazy.txt': () => 'lazy content',        // lazy sync
    '/async.txt': async () => fetchData(),    // lazy async
  },
  env: { USER: 'agent', HOME: '/home/agent' },
  cwd: '/',
  executionLimits: {
    maxCommandCount: 10000,
    maxExecutionTimeSecs: 30,
  },
  customCommands: [myCommand],
});

// Execute commands
const result = await bash.exec('echo hello | tr a-z A-Z');
// { stdout: "HELLO\n", stderr: "", exitCode: 0 }

// Per-exec overrides
const result2 = await bash.exec('cat /data.txt', {
  env: { LANG: 'en_US.UTF-8' },
  cwd: '/data',
  stdin: 'input data',
});

// Direct VFS access
bash.fs.writeFileSync('/output.txt', 'content');
const data = bash.fs.readFileSync('/output.txt');
```

### Custom Commands

```typescript
import { defineCommand } from 'rust-bash';

const fetch = defineCommand('fetch', async (args, ctx) => {
  const url = args[0];
  const response = await globalThis.fetch(url);
  return { stdout: await response.text(), stderr: '', exitCode: 0 };
});

const bash = await Bash.create(createBackend, {
  customCommands: [fetch],
});
```

### Package Exports

| Export | Description |
|--------|-------------|
| `Bash` | Main class — `Bash.create(backend, options)` |
| `defineCommand` | Create custom commands |
| `bashToolDefinition` | JSON Schema tool definition for AI integration |
| `createBashToolHandler` | Factory for tool handlers |
| `formatToolForProvider` | Format tools for OpenAI, Anthropic, MCP |
| `handleToolCall` | Multi-tool dispatcher |
| `initWasm` / `createWasmBackend` | WASM backend |
| `tryLoadNative` / `createNativeBackend` | Native addon backend |

## AI SDK Tool Definition

For use with OpenAI, Anthropic, and other function-calling LLM APIs. Available via both the TypeScript npm package and the Rust CLI's MCP server mode.

### TypeScript: Framework-Agnostic Primitives

`rust-bash` exports JSON Schema tool definitions and a handler factory that work with **any** AI agent framework — no framework dependencies required.

```typescript
import { bashToolDefinition, createBashToolHandler, formatToolForProvider, createNativeBackend } from 'rust-bash';

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

const result = await handler({ command: 'grep hello /data.txt' });
// { stdout: 'hello world\n', stderr: '', exitCode: 0 }

// Format for specific providers (thin wrappers, no dependencies)
const openaiTool = formatToolForProvider(bashToolDefinition, 'openai');
// { type: "function", function: { name: "bash", description: "...", parameters: {...} } }

const anthropicTool = formatToolForProvider(bashToolDefinition, 'anthropic');
// { name: "bash", description: "...", input_schema: {...} }

const mcpTool = formatToolForProvider(bashToolDefinition, 'mcp');
// { name: "bash", description: "...", inputSchema: {...} }
```

Additional exports for agent loops:

- `handleToolCall(bash, toolName, args)` — dispatches `bash`, `readFile`/`read_file`, `writeFile`/`write_file`, `listDirectory`/`list_directory` tool calls (supports both camelCase and snake_case)
- `writeFileToolDefinition`, `readFileToolDefinition`, `listDirectoryToolDefinition` — JSON Schema definitions for file operation tools

### Recipe: OpenAI

```typescript
import OpenAI from 'openai';
import { createBashToolHandler, formatToolForProvider, bashToolDefinition, createNativeBackend } from 'rust-bash';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });

const response = await openai.chat.completions.create({
  model: 'gpt-4o',
  tools: [formatToolForProvider(bashToolDefinition, 'openai')],
  messages: [{ role: 'user', content: 'List files in /data' }],
});

for (const toolCall of response.choices[0].message.tool_calls ?? []) {
  const result = await handler(JSON.parse(toolCall.function.arguments));
}
```

### Recipe: Anthropic

```typescript
import Anthropic from '@anthropic-ai/sdk';
import { createBashToolHandler, formatToolForProvider, bashToolDefinition, createNativeBackend } from 'rust-bash';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });

const response = await anthropic.messages.create({
  model: 'claude-sonnet-4-20250514',
  max_tokens: 1024,
  tools: [formatToolForProvider(bashToolDefinition, 'anthropic')],
  messages: [{ role: 'user', content: 'List files in /data' }],
});

for (const block of response.content) {
  if (block.type === 'tool_use') {
    const result = await handler(block.input);
  }
}
```

### Recipe: Vercel AI SDK

```typescript
import { tool } from 'ai';
import { z } from 'zod';
import { createBashToolHandler, createNativeBackend } from 'rust-bash';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });
const bashTool = tool({
  description: 'Execute bash commands in a sandbox',
  parameters: z.object({ command: z.string() }),
  execute: async ({ command }) => handler({ command }),
});
```

### Recipe: LangChain.js

```typescript
import { tool } from '@langchain/core/tools';
import { z } from 'zod';
import { createBashToolHandler, createNativeBackend } from 'rust-bash';

const { handler, definition } = createBashToolHandler(createNativeBackend, { files: myFiles });
const bashTool = tool(
  async ({ command }) => JSON.stringify(await handler({ command })),
  { name: definition.name, description: definition.description, schema: z.object({ command: z.string() }) },
);
```

See [AI Agent Tool Recipe](../recipes/ai-agent-tool.md) for complete examples with full agent loops.

### MCP Server Mode

The CLI binary includes a built-in MCP (Model Context Protocol) server for direct integration with Claude Desktop, Cursor, VS Code, Windsurf, Cline, and the OpenAI Agents SDK:

```bash
rust-bash --mcp
```

Exposed tools: `bash`, `write_file`, `read_file`, `list_directory`.

Configuration for Claude Desktop (`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

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

Configuration for VS Code (`.vscode/mcp.json`):

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

The MCP server maintains a stateful shell session across all tool calls — variables, files, and working directory persist between invocations.

See [MCP Server Setup](../recipes/mcp-server.md) for detailed setup instructions for all supported clients.

### Tool Schema

```json
{
  "type": "function",
  "function": {
    "name": "bash",
    "description": "Execute bash commands in a sandboxed environment with an in-memory filesystem.",
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
}
```

### Rust Integration Pattern

```rust
// Create sandbox once per agent session
let mut shell = RustBashBuilder::new()
    .files(project_files)
    .build()
    .unwrap();

// In the agent tool dispatch loop:
match tool_call.name.as_str() {
    "bash" => {
        let command = tool_call.arguments["command"].as_str().unwrap();
        let result = shell.exec(command)?;
        format!("stdout:\n{}\nstderr:\n{}\nexit_code: {}", 
                result.stdout, result.stderr, result.exit_code)
    }
    _ => { /* other tools */ }
}
```

## Browser Integration (WASM)

rust-bash runs in the browser via WebAssembly. The `rust-bash` npm package provides a browser entry point that loads the WASM binary.

### Architecture

The showcase website at `examples/website/` demonstrates the full browser integration:

1. **xterm.js** renders a terminal in the browser
2. **rust-bash WASM** (or a development mock) executes commands
3. An **AI agent** (via Cloudflare Worker → Gemini API) can request tool calls
4. Tool calls execute **locally** in the browser — no server roundtrip for bash

### Key Concepts

- **Shared state**: The user and agent share the same bash instance and VFS. Files created by the agent are visible to the user and vice versa.
- **Client-side execution**: All bash commands run locally in the WASM module. The only network call is to the LLM proxy for the `agent` command.
- **Cached responses**: The initial demo uses a hand-crafted `AgentEvent[]` array, avoiding API calls on first load.

### Usage

```typescript
import { Bash, initWasm, createWasmBackend } from 'rust-bash/browser';

// Initialize WASM module
await initWasm();

// Create a bash instance with preloaded files
const bash = await Bash.create(createWasmBackend, {
  files: { '/hello.txt': 'Hello from WASM!' },
  cwd: '/home/user',
});

const result = await bash.exec('cat /hello.txt');
console.log(result.stdout); // "Hello from WASM!"
```
