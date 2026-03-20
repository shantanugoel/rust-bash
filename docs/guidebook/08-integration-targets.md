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
    "max_call_depth": 100,
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
