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

## CLI Binary **(planned)**

A standalone binary for command-line usage (Milestone M5.1).

> **Note**: An interactive REPL is already available as a runnable example:
> `cargo run --example shell` — useful for manual exploration during development.
> It supports `--env KEY=VAL` and `--files ./dir` to seed the sandbox from the host.
> This is a development tool, not the production CLI binary described below.

```bash
# Execute a command
rust-bash -c 'echo hello | wc -c'

# Seed files from disk
rust-bash --files /data:/app/data.txt --files /config:/app/config.json -c 'cat /app/data.txt'

# Set environment
rust-bash --env USER=agent --env HOME=/home/agent -c 'echo $USER'

# Interactive REPL
rust-bash

# JSON output for machine consumption
rust-bash --json -c 'echo hello'
# {"stdout":"hello\n","stderr":"","exit_code":0}

# Read commands from stdin
echo 'echo hello' | rust-bash
```

The CLI binary compiles as a single static binary with zero runtime dependencies.

## C FFI **(planned)**

For embedding in Python, Go, Ruby, or any language with C interop.

### API

```c
typedef struct RustBash RustBash;
typedef struct ExecResult {
    const char* stdout_ptr;  int stdout_len;
    const char* stderr_ptr;  int stderr_len;
    int exit_code;
} ExecResult;

// Create a sandbox from JSON configuration
RustBash* rust_bash_create(const char* config_json);

// Execute a command
ExecResult* rust_bash_exec(RustBash* sb, const char* command);

// Free resources
void rust_bash_result_free(ExecResult* result);
void rust_bash_free(RustBash* sb);
```

### Configuration via JSON

Config is passed as JSON for maximum language interop:

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
    "max_execution_time_secs": 30
  }
}
```

### Memory Ownership

- `rust_bash_create` returns a heap-allocated sandbox; caller must call `rust_bash_free`
- `rust_bash_exec` returns a heap-allocated result; caller must call `rust_bash_result_free`
- String pointers in `ExecResult` are valid until `rust_bash_result_free` is called

### Python Example

```python
import ctypes

lib = ctypes.CDLL("./librust_bash.so")

# Define types
lib.rust_bash_create.restype = ctypes.c_void_p
lib.rust_bash_exec.restype = ctypes.POINTER(ExecResult)

sb = lib.rust_bash_create(b'{"files":{"/data.txt":"hello"}}')
result = lib.rust_bash_exec(sb, b'cat /data.txt')
print(result.contents.stdout_ptr[:result.contents.stdout_len])

lib.rust_bash_result_free(result)
lib.rust_bash_free(sb)
```

## WASM Target **(planned)**

For browser and edge runtime embedding.

### Build

```bash
cargo build --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/release/rust_bash.wasm --out-dir pkg
```

### JavaScript API

```javascript
import { createSandbox } from 'rust-bash-wasm';

const sandbox = createSandbox({
  files: { '/data.txt': 'content' },
  env: { USER: 'agent' },
});

const result = shell.exec('cat /data.txt | grep content');
console.log(result.stdout);  // "content\n"
```

### Compatibility Notes

- brush-parser already compiles to `wasm32-unknown-unknown`
- The interpreter and VFS are pure Rust with no OS dependencies
- `SystemTime` needs stubbing for WASM (use a monotonic counter or injected clock)
- Networking (`curl`) must be feature-gated out or use `fetch()` API
- Estimated bundle size: ~800KB–1.2MB gzipped

### npm Package

Distributed as an npm package with TypeScript type definitions:

```typescript
interface RustBashOptions {
  files?: Record<string, string>;
  env?: Record<string, string>;
  cwd?: string;
  limits?: Partial<ExecutionLimits>;
}

interface ExecResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

function createSandbox(options?: RustBashOptions): RustBash;

interface RustBash {
  exec(command: string): ExecResult;
  free(): void;
}
```

## AI SDK Tool Definition **(planned)**

For use with OpenAI, Anthropic, and other function-calling LLM APIs.

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

### Integration Pattern

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

### Vercel AI SDK Compatibility **(planned)**

A TypeScript wrapper using the WASM target for `@vercel/ai` compatibility:

```typescript
import { createBashTool } from 'rust-bash-ai';

const tools = {
  bash: createBashTool({
    files: { '/project/data.csv': csvContent },
    limits: { maxCommandCount: 1000 },
  }),
};
```
