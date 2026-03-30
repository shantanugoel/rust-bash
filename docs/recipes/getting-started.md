# Getting Started: Embedding rust-bash in a Rust Application

## Goal

Create a sandboxed bash shell, execute scripts, and inspect results — all from Rust code with no host filesystem access.

## Minimal Example

```rust
use rust_bash::RustBashBuilder;

fn main() {
    let mut shell = RustBashBuilder::new().build().unwrap();

    let result = shell.exec("echo 'Hello from rust-bash!'").unwrap();
    assert_eq!(result.stdout, "Hello from rust-bash!\n");
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stderr, "");
}
```

`RustBashBuilder::new().build()` gives you a shell with:
- An empty in-memory filesystem (just a root `/` directory)
- No environment variables
- Working directory at `/`
- All built-in commands registered (62 commands + 18 interpreter builtins)
- Default execution limits (10k commands, 30s timeout, etc.)

## Pre-populating Files and Environment

Most real use cases need seed data. Use the builder to set up files, environment variables, and a working directory:

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/etc/config.json".into(), br#"{"debug": true}"#.to_vec()),
        ("/app/script.sh".into(), b"echo running; cat /etc/config.json".to_vec()),
    ]))
    .env(HashMap::from([
        ("HOME".into(), "/home/user".into()),
        ("APP_ENV".into(), "production".into()),
    ]))
    .cwd("/app")
    .build()
    .unwrap();

// Source a script file
let result = shell.exec("source /app/script.sh").unwrap();
assert!(result.stdout.contains("running"));
assert!(result.stdout.contains("debug"));
```

Parent directories are created automatically — `/etc/` and `/app/` don't need to exist beforehand.

## Inspecting Results

Every `exec()` call returns an `ExecResult` with three fields:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

let result = shell.exec("echo hello; echo oops >&2; exit 42").unwrap();

println!("stdout: {:?}", result.stdout);    // "hello\n"
println!("stderr: {:?}", result.stderr);    // "oops\n"
println!("exit code: {}", result.exit_code); // 42
```

You can also query shell state after execution:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

shell.exec("cd /tmp && FOO=bar").unwrap();

println!("cwd: {}", shell.cwd());              // "/tmp"
println!("last exit: {}", shell.last_exit_code()); // 0
println!("should exit: {}", shell.should_exit());  // false
```

## Error Handling

`exec()` returns `Result<ExecResult, RustBashError>`. The error types are:

- `RustBashError::Parse` — syntax error in the script
- `RustBashError::Execution` — runtime error (e.g., readonly variable assignment)
- `RustBashError::LimitExceeded` — a configured limit was hit
- `RustBashError::Vfs` — filesystem error
- `RustBashError::Timeout` — execution time exceeded

```rust
use rust_bash::{RustBashBuilder, RustBashError};

let mut shell = RustBashBuilder::new().build().unwrap();

match shell.exec("echo 'unterminated") {
    Ok(result) => println!("stdout: {}", result.stdout),
    Err(RustBashError::Parse(msg)) => eprintln!("Parse error: {msg}"),
    Err(e) => eprintln!("Other error: {e}"),
}
```

Note: a command returning a non-zero exit code is **not** an error — it's a normal `Ok(ExecResult)` with `exit_code != 0`. Parse and limit errors are the exceptional cases.

## Listing Available Commands

```rust
use rust_bash::RustBashBuilder;

let shell = RustBashBuilder::new().build().unwrap();
let mut names = shell.command_names();
names.sort();
println!("Available commands: {}", names.join(", "));
// echo, cat, grep, sed, awk, jq, find, sort, ... (80+ commands)
```

## Next Steps

- [Custom Commands](custom-commands.md) — register your own domain-specific commands
- [Execution Limits](execution-limits.md) — configure safety bounds
- [Filesystem Backends](filesystem-backends.md) — use OverlayFs, ReadWriteFs, or MountableFs

---

## Getting Started with TypeScript / npm

If you're using TypeScript or JavaScript, see the npm package:

```bash
npm install @shantanugoel/rust-bash
```

### Minimal Example (Node.js)

```typescript
import { Bash, tryLoadNative, createNativeBackend, initWasm, createWasmBackend } from '@shantanugoel/rust-bash';

// Auto-detect backend: native addon (fast) or WASM (universal)
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
console.log(result.stdout);   // "hello world\n"
console.log(result.exitCode); // 0
```

### Minimal Example (Browser)

```typescript
import { Bash, initWasm, createWasmBackend } from '@shantanugoel/rust-bash/browser';

await initWasm();
const bash = await Bash.create(createWasmBackend, {
  files: { '/hello.txt': 'Hello from WASM!' },
});

const result = await bash.exec('cat /hello.txt');
console.log(result.stdout); // "Hello from WASM!\n"
```

### Pre-populating Files

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    '/data.json': '{"name": "world"}',
    '/config.yml': 'debug: true',
    '/lazy.txt': () => 'resolved on first read',           // lazy sync
    '/async.txt': async () => fetchDataFromAPI(),           // lazy async
  },
  env: { HOME: '/home/user', APP_ENV: 'production' },
  cwd: '/app',
});
```

### Inspecting Results

```typescript
const result = await bash.exec('echo hello; echo oops >&2; exit 42');
console.log(result.stdout);   // "hello\n"
console.log(result.stderr);   // "oops\n"
console.log(result.exitCode); // 42
```

### Direct Filesystem Access

```typescript
bash.fs.writeFileSync('/output.txt', 'content');
const data = bash.fs.readFileSync('/output.txt');
const exists = bash.fs.existsSync('/output.txt');
bash.fs.mkdirSync('/dir', { recursive: true });
const entries = bash.fs.readdirSync('/');
```

### TypeScript Next Steps

- [npm package README](../../packages/core/README.md) — full TypeScript API reference
- [Embedding in an AI Agent](ai-agent-tool.md) — use as a tool for LLM function calling
- [MCP Server](mcp-server.md) — built-in MCP server for Claude Desktop, Cursor, VS Code
