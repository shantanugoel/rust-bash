# rust-bash

A sandboxed bash interpreter built in Rust. Execute bash scripts safely with a virtual filesystem — no containers, no VMs, no host access.

> **Status: Pre-alpha / Milestones 1–5 Complete** — Core interpreter, text processing,
> execution safety, filesystem backends, CLI binary, C FFI, WASM target, npm package, and AI SDK integration are implemented.

### 🌐 [Try it in the browser →](https://rustbash.dev)

Interactive showcase with 80+ commands running via WASM. Includes an AI agent you can talk to. See [`examples/website/`](examples/website/) for the source.

## Highlights

- **Virtual filesystem** — all file operations happen in memory by default. No host files are touched.
- **80 commands** — echo, cat, grep, awk, sed, jq, find, sort, diff, curl, and many more.
- **Full bash syntax** — pipelines, redirections, variables, control flow, functions, command substitution, globs, brace expansion, arithmetic, here-documents, case statements.
- **Execution limits** — 10 configurable bounds (time, commands, loops, output size, call depth, string length, glob results, substitution depth, heredoc size, brace expansion).
- **Network policy** — sandboxed `curl` with URL allow-lists, method restrictions, redirect and response-size limits.
- **Multiple filesystem backends** — InMemoryFs (default), OverlayFs (copy-on-write), ReadWriteFs (passthrough), MountableFs (composite).
- **npm package** — `@rust-bash/core` with TypeScript types, native Node.js addon, and WASM support.
- **AI tool integration** — framework-agnostic JSON Schema tool definitions for OpenAI, Anthropic, Vercel AI SDK, LangChain.js.
- **MCP server** — built-in Model Context Protocol server for Claude Desktop, Cursor, VS Code.
- **Embeddable** — use as a Rust crate with a builder API. Custom commands via the `VirtualCommand` trait.
- **CLI binary** — standalone `rust-bash` command with `-c`, `--files`, `--env`, `--cwd`, `--json` flags, MCP server mode, and an interactive REPL.

## Installation

### npm (TypeScript / JavaScript)

```bash
npm install @rust-bash/core
```

### Build from source (Rust)

```bash
git clone https://github.com/shantanugoel/rust-bash.git
cd rust-bash
cargo build --release
# Binary is at target/release/rust-bash
```

### Install via Cargo

```bash
cargo install --path .
```

## Quick Start (TypeScript)

```typescript
import { Bash, tryLoadNative, createNativeBackend, initWasm, createWasmBackend } from '@rust-bash/core';

// Auto-detect backend: native addon (fast) or WASM (universal)
let createBackend;
if (await tryLoadNative()) {
  createBackend = createNativeBackend;
} else {
  await initWasm();
  createBackend = createWasmBackend;
}

const bash = await Bash.create(createBackend, {
  files: {
    '/data.json': '{"name": "world"}',
    '/script.sh': 'echo "Hello, $(jq -r .name /data.json)!"',
  },
  env: { USER: 'agent' },
});

const result = await bash.exec('bash /script.sh');
console.log(result.stdout);   // "Hello, world!\n"
console.log(result.exitCode); // 0
```

## Quick Start (Rust)

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/data.txt".into(), b"hello world".to_vec()),
    ]))
    .env(HashMap::from([
        ("USER".into(), "agent".into()),
    ]))
    .build()
    .unwrap();

let result = shell.exec("cat /data.txt | grep hello").unwrap();
assert_eq!(result.stdout, "hello world\n");
assert_eq!(result.exit_code, 0);
```

## Custom Commands

### TypeScript

```typescript
import { Bash, defineCommand } from '@rust-bash/core';

const fetch = defineCommand('fetch', async (args, ctx) => {
  const url = args[0];
  const response = await globalThis.fetch(url);
  return { stdout: await response.text(), stderr: '', exitCode: 0 };
});

const bash = await Bash.create(createBackend, {
  customCommands: [fetch],
});

await bash.exec('fetch https://api.example.com/data');
```

### Rust

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};

struct MyCommand;

impl VirtualCommand for MyCommand {
    fn name(&self) -> &str { "my-cmd" }
    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: format!("got {} args\n", args.len()),
            ..Default::default()
        }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(MyCommand))
    .build()
    .unwrap();

let result = shell.exec("my-cmd foo bar").unwrap();
assert_eq!(result.stdout, "got 2 args\n");
```

## AI Tool Integration

`@rust-bash/core` exports framework-agnostic tool primitives — use with any AI agent framework:

```typescript
import {
  bashToolDefinition,
  createBashToolHandler,
  formatToolForProvider,
  createNativeBackend,
} from '@rust-bash/core';

const { handler } = createBashToolHandler(createNativeBackend, {
  files: { '/data.txt': 'hello world' },
  maxOutputLength: 10000,
});

// Format for your provider
const openaiTool = formatToolForProvider(bashToolDefinition, 'openai');
const anthropicTool = formatToolForProvider(bashToolDefinition, 'anthropic');

// Handle tool calls from the LLM
const result = await handler({ command: 'grep hello /data.txt' });
// { stdout: 'hello world\n', stderr: '', exitCode: 0 }
```

### OpenAI

```typescript
import OpenAI from 'openai';
import { createBashToolHandler, formatToolForProvider, bashToolDefinition, createNativeBackend } from '@rust-bash/core';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });
const openai = new OpenAI();

const response = await openai.chat.completions.create({
  model: 'gpt-4o',
  tools: [formatToolForProvider(bashToolDefinition, 'openai')],
  messages: [{ role: 'user', content: 'Count lines in /data.txt' }],
});

for (const toolCall of response.choices[0].message.tool_calls ?? []) {
  const result = await handler(JSON.parse(toolCall.function.arguments));
}
```

### Anthropic

```typescript
import Anthropic from '@anthropic-ai/sdk';
import { createBashToolHandler, formatToolForProvider, bashToolDefinition, createNativeBackend } from '@rust-bash/core';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });
const anthropic = new Anthropic();

const response = await anthropic.messages.create({
  model: 'claude-sonnet-4-20250514',
  max_tokens: 1024,
  tools: [formatToolForProvider(bashToolDefinition, 'anthropic')],
  messages: [{ role: 'user', content: 'Count lines in /data.txt' }],
});

for (const block of response.content) {
  if (block.type === 'tool_use') {
    const result = await handler(block.input);
  }
}
```

### Vercel AI SDK

```typescript
import { tool } from 'ai';
import { z } from 'zod';
import { createBashToolHandler, createNativeBackend } from '@rust-bash/core';

const { handler } = createBashToolHandler(createNativeBackend, { files: myFiles });
const bashTool = tool({
  description: 'Execute bash commands in a sandbox',
  parameters: z.object({ command: z.string() }),
  execute: async ({ command }) => handler({ command }),
});
```

### LangChain.js

```typescript
import { tool } from '@langchain/core/tools';
import { z } from 'zod';
import { createBashToolHandler, createNativeBackend } from '@rust-bash/core';

const { handler, definition } = createBashToolHandler(createNativeBackend, { files: myFiles });
const bashTool = tool(
  async ({ command }) => JSON.stringify(await handler({ command })),
  { name: definition.name, description: definition.description, schema: z.object({ command: z.string() }) },
);
```

See [AI Agent Tool Recipe](docs/recipes/ai-agent-tool.md) for complete agent loop examples.

## MCP Server

The CLI binary includes a built-in [Model Context Protocol](https://modelcontextprotocol.io/) server:

```bash
rust-bash --mcp
```

Exposed tools: `bash`, `write_file`, `read_file`, `list_directory`. State persists across calls.

### Claude Desktop

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

### VS Code (GitHub Copilot)

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

See [MCP Server Setup](docs/recipes/mcp-server.md) for Cursor, Windsurf, Cline, and other clients.

## Browser / WASM

```typescript
import { Bash, initWasm, createWasmBackend } from '@rust-bash/core/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend, {
  files: { '/hello.txt': 'Hello from WASM!' },
});

const result = await bash.exec('cat /hello.txt');
console.log(result.stdout); // "Hello from WASM!\n"
```

## Performance

| Feature | just-bash | @rust-bash/core |
|---------|-----------|-----------------|
| Language | Pure TypeScript | Rust → WASM + native addon |
| Performance | JS-speed | Near-native (native addon) / WASM |
| API | `new Bash(opts)` | `Bash.create(backend, opts)` |
| Custom commands | `defineCommand()` | `defineCommand()` (same API) |
| AI integration | Vercel AI SDK only | Framework-agnostic (OpenAI, Anthropic, Vercel, LangChain) |
| MCP server | ❌ | ✅ Built-in (`rust-bash --mcp`) |
| Browser | ✅ | ✅ (WASM) |
| Node.js native | ❌ | ✅ (napi-rs) |
| C FFI | ❌ | ✅ (shared library) |
| Filesystem backends | In-memory only | InMemoryFs, OverlayFs, ReadWriteFs, MountableFs |
| Execution limits | ✅ | ✅ (10 configurable bounds) |
| Network policy | ❌ | ✅ (URL allow-list, method restrictions) |

## CLI Binary

```bash
# Execute a command
rust-bash -c 'echo hello | wc -c'

# Seed files from host disk into the virtual filesystem
rust-bash --files /path/to/data.txt:/data.txt -c 'cat /data.txt'
rust-bash --files /path/to/dir -c 'ls /'

# Set environment variables
rust-bash --env USER=agent --env HOME=/home/agent -c 'echo $USER'

# Set working directory
rust-bash --cwd /app -c 'pwd'

# JSON output for machine consumption
rust-bash --json -c 'echo hello'
# {"stdout":"hello\n","stderr":"","exit_code":0}

# Execute a script file with positional arguments
rust-bash script.sh arg1 arg2

# Read commands from stdin
echo 'echo hello' | rust-bash

# MCP server mode
rust-bash --mcp

# Interactive REPL (starts when no command/script/stdin is given)
rust-bash
```

### Interactive REPL

When launched without `-c`, a script file, or piped stdin, `rust-bash` starts an
interactive REPL with readline support:

- **Colored prompt** — `rust-bash:{cwd}$ ` reflecting the current directory, green (exit 0) or red (non-zero last exit)
- **Tab completion** — completes built-in command names
- **Multi-line input** — incomplete constructs (e.g., `if true; then`) wait for more input
- **History** — persists across sessions in `~/.rust_bash_history`
- **Ctrl-C** — cancels the current input line
- **Ctrl-D** — exits the REPL with the last command's exit code
- **`exit [N]`** — exits with code N (default 0)

## Use Cases

- **AI agent tools** — give LLMs a bash sandbox without container overhead
- **Code sandboxes** — run user-submitted scripts safely
- **Testing** — deterministic bash execution with a controlled filesystem
- **Embedded scripting** — add bash scripting to Rust applications
- **MCP server** — provide bash execution to Claude Desktop, Cursor, VS Code

## Built-in Commands

### Registered commands (62)

| Category | Commands |
|----------|----------|
| **Core** | `echo`, `cat`, `true`, `false`, `pwd`, `touch`, `mkdir`, `ls`, `test`, `[` |
| **File ops** | `cp`, `mv`, `rm`, `tee`, `stat`, `chmod`, `ln` |
| **Text** | `grep`, `sort`, `uniq`, `cut`, `head`, `tail`, `wc`, `tr`, `rev`, `fold`, `nl`, `printf`, `paste`, `tac`, `comm`, `join`, `fmt`, `column`, `expand`, `unexpand` |
| **Text processing** | `sed`, `awk`, `jq`, `diff` |
| **Navigation** | `realpath`, `basename`, `dirname`, `tree`, `find` |
| **Utilities** | `expr`, `date`, `sleep`, `seq`, `env`, `printenv`, `which`, `base64`, `md5sum`, `sha256sum`, `whoami`, `hostname`, `uname`, `yes`, `xargs` |
| **Network** | `curl` |

### Interpreter builtins (18)

`exit`, `cd`, `export`, `unset`, `set`, `shift`, `readonly`, `declare`, `read`, `eval`, `source` / `.`, `break`, `continue`, `:`, `let`, `local`, `return`, `trap`

Additionally, `if`/`then`/`elif`/`else`/`fi`, `for`/`while`/`until`/`do`/`done`, `case`/`esac`, `((...))`, and `[[ ]]` are handled as shell syntax by the interpreter.

## Configuration (Rust)

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits, NetworkPolicy};
use std::collections::HashMap;
use std::time::Duration;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/app/script.sh".into(), b"echo hello".to_vec()),
    ]))
    .env(HashMap::from([
        ("HOME".into(), "/home/agent".into()),
    ]))
    .cwd("/app")
    .execution_limits(ExecutionLimits {
        max_command_count: 1_000,
        max_execution_time: Duration::from_secs(5),
        ..Default::default()
    })
    .network_policy(NetworkPolicy {
        enabled: true,
        allowed_url_prefixes: vec!["https://api.example.com/".into()],
        ..Default::default()
    })
    .build()
    .unwrap();
```

### Execution limits defaults

| Limit | Default |
|-------|---------|
| `max_call_depth` | 100 |
| `max_command_count` | 10,000 |
| `max_loop_iterations` | 10,000 |
| `max_execution_time` | 30 s |
| `max_output_size` | 10 MB |
| `max_string_length` | 10 MB |
| `max_glob_results` | 100,000 |
| `max_substitution_depth` | 50 |
| `max_heredoc_size` | 10 MB |
| `max_brace_expansion` | 10,000 |

## Filesystem Backends

| Backend | Description |
|---------|-------------|
| `InMemoryFs` | Default. All data in memory. Zero host access. |
| `OverlayFs` | Copy-on-write over a real directory. Reads from disk, writes stay in memory. |
| `ReadWriteFs` | Passthrough to real filesystem. For trusted execution. |
| `MountableFs` | Compose backends at different mount points. |

### OverlayFs — Read real files, sandbox writes

```rust
use rust_bash::{RustBashBuilder, OverlayFs};
use std::sync::Arc;

// Reads from ./my_project on disk; writes stay in memory
let overlay = OverlayFs::new("./my_project").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(overlay))
    .cwd("/")
    .build()
    .unwrap();

let result = shell.exec("cat /src/main.rs").unwrap();    // reads from disk
shell.exec("echo patched > /src/main.rs").unwrap();       // writes to memory only
```

### ReadWriteFs — Direct filesystem access

```rust
use rust_bash::{RustBashBuilder, ReadWriteFs};
use std::sync::Arc;

// Restricted to /tmp/sandbox (chroot-like)
let rwfs = ReadWriteFs::with_root("/tmp/sandbox").unwrap();
let mut shell = RustBashBuilder::new()
    .fs(Arc::new(rwfs))
    .cwd("/")
    .build()
    .unwrap();

shell.exec("echo hello > /output.txt").unwrap();  // writes to /tmp/sandbox/output.txt
```

### MountableFs — Combine backends at mount points

```rust
use rust_bash::{RustBashBuilder, InMemoryFs, MountableFs, OverlayFs};
use std::sync::Arc;

let mountable = MountableFs::new()
    .mount("/", Arc::new(InMemoryFs::new()))                                // in-memory root
    .mount("/project", Arc::new(OverlayFs::new("./myproject").unwrap()))    // overlay on real project
    .mount("/tmp", Arc::new(InMemoryFs::new()));                            // separate temp space

let mut shell = RustBashBuilder::new()
    .fs(Arc::new(mountable))
    .cwd("/")
    .build()
    .unwrap();

shell.exec("cat /project/README.md").unwrap();   // reads from disk
shell.exec("echo scratch > /tmp/work").unwrap(); // writes to in-memory /tmp
```

## C FFI

rust-bash can be used from any language with C FFI support (Python, Go, Ruby, etc.) via a shared library.

### Build the shared library

```bash
cargo build --features ffi --release
# Output: target/release/librust_bash.so (Linux), .dylib (macOS), .dll (Windows)
# Header: include/rust_bash.h
```

### Minimal C example

```c
#include "rust_bash.h"
#include <stdio.h>

int main(void) {
    struct RustBash *sb = rust_bash_create(NULL);
    struct ExecResult *r = rust_bash_exec(sb, "echo hello world");
    printf("%.*s", r->stdout_len, r->stdout_ptr);
    rust_bash_result_free(r);
    rust_bash_free(sb);
    return 0;
}
```

For complete Python and Go examples, see [`examples/ffi/`](examples/ffi/). For the full FFI guide, see the [FFI Usage recipe](docs/recipes/ffi-usage.md).

## Public API (Rust)

| Type | Description |
|------|-------------|
| `RustBashBuilder` | Builder for configuring and constructing a shell instance |
| `RustBash` | The shell instance — call `.exec(script)` to run commands |
| `ExecResult` | Returned by `exec()`: `stdout`, `stderr`, `exit_code` |
| `ExecutionLimits` | Configurable resource bounds |
| `NetworkPolicy` | URL allow-list and HTTP method restrictions for `curl` |
| `VirtualCommand` | Trait for registering custom commands |
| `CommandContext` | Passed to command implementations (fs, cwd, env, stdin, limits) |
| `CommandResult` | Returned by command implementations |
| `RustBashError` | Top-level error: `Parse`, `Execution`, `LimitExceeded`, `Network`, `Vfs`, `Timeout` |
| `VfsError` | Filesystem errors: `NotFound`, `AlreadyExists`, `PermissionDenied`, etc. |
| `Variable` | A shell variable with `value`, `exported`, `readonly` metadata |
| `ShellOpts` | Shell option flags: `errexit`, `nounset`, `pipefail`, `xtrace` |
| `ExecutionCounters` | Per-`exec()` resource usage counters |
| `InterpreterState` | Full mutable shell state (advanced: direct inspection/manipulation) |
| `ExecCallback` | Callback type for sub-command execution (`xargs`, `find -exec`) |
| `InMemoryFs` | In-memory filesystem backend |
| `OverlayFs` | Copy-on-write overlay backend |
| `ReadWriteFs` | Real filesystem passthrough backend |
| `MountableFs` | Composite backend with path-based mount delegation |
| `VirtualFs` | Trait for filesystem backends |

## Documentation

- [Guidebook](docs/guidebook/) — architecture, design, and implementation details
- [Recipes](docs/recipes/) — task-oriented guides for common use cases
- [npm package README](packages/core/README.md) — TypeScript API reference

## Roadmap

The following milestones track the project's progress:

- ✅ **Milestone 1–4**: Core interpreter, text processing, execution safety, filesystem backends
- ✅ **Milestone 5.1**: Standalone CLI binary — interactive REPL, `-c` commands, script files, stdin piping, `--json` output
- ✅ **Milestone 5.2**: C FFI — shared library, generated C header, JSON config, 6 exported functions
- ✅ **Milestone 5.3**: WASM target — `wasm32-unknown-unknown`, npm package `@rust-bash/core` with TypeScript types
- ✅ **Milestone 5.4**: AI SDK integration — framework-agnostic tool definitions, MCP server, documented adapters
- ✅ **Milestone 6.12**: Differential testing — 2,731 test cases across three suites (comparison fixtures, command spec tests, upstream Oils bash conformance tests)
- Planned: Shell language completeness — arrays, shopt, process substitution (M6)
- Planned: Command coverage — `--help` for all commands, missing utilities (M7)
- Planned: Embedded runtimes — SQLite, yq, Python, JavaScript (M8)
- Planned: Platform features — cancellation, lazy files, AST transforms, fuzz testing (M9)

## License

MIT
