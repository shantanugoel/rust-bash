# rust-bash

A sandboxed bash environment built in Rust. Execute bash scripts safely with a virtual filesystem — no containers, no VMs, no runtime dependencies.

> ⚠️ **Status: Pre-alpha / Milestone 4 Complete** — Core shell interpreter with full text processing,
> execution safety, and multiple filesystem backends. Supports variable expansion, redirections,
> control flow, command substitution, arithmetic, functions, globs, brace expansion, here-documents,
> and 70+ built-in commands including text processing tools (grep, sed, awk, jq, diff) and network
> access (curl with policy controls). All 10 execution limits are enforced with structured errors.
> Filesystem backends: InMemoryFs (default), OverlayFs (copy-on-write), ReadWriteFs (passthrough),
> MountableFs (composite).

## Design Goals

- **Virtual filesystem** — all file operations happen in memory. No host files are touched.
- **70+ commands** — echo, cat, grep, awk, sed, jq, find, sort, curl, and many more.
- **Full bash syntax** — pipelines, redirections, variables, control flow, functions, command substitution, globs, arithmetic.
- **Execution limits** — configurable bounds on time, commands, loops, and output size.
- **Zero dependencies** — ships as a static binary or embeddable library.
- **Multiple targets** — Rust crate, C FFI (for Python/Go/Ruby), WASM (for browsers), CLI binary.

## Quick Start

### As a Rust crate

```rust
use rust_bash::RustBash;

let mut shell = RustBash::builder()
    .files([("/data.txt", "hello world")])
    .env([("USER", "agent")])
    .build();

let result = shell.exec("cat /data.txt | grep hello").unwrap();
assert_eq!(result.stdout, "hello world\n");
assert_eq!(result.exit_code, 0);
```

### As a CLI

```bash
# Run a command
rust-bash -c 'echo "hello world" | wc -w'

# Seed files from disk
rust-bash --files ./data.csv:/app/data.csv -c 'wc -l /app/data.csv'

# Interactive mode
rust-bash
```

### As a WASM module

```javascript
import { createSandbox } from 'rust-bash-wasm';

const sandbox = createSandbox({ files: { '/data.txt': 'content' } });
const result = shell.exec('cat /data.txt');
console.log(result.stdout); // "content\n"
```

## Use Cases

- **AI agent tools** — give LLMs a bash sandbox without container overhead
- **Code sandboxes** — run user-submitted scripts safely
- **Education** — bash playground in the browser via WASM
- **Testing** — deterministic bash execution with a controlled filesystem

## Configuration

```rust
use rust_bash::{RustBash, ExecutionLimits, NetworkPolicy};
use std::time::Duration;

let mut shell = RustBash::builder()
    .files([("/app/script.sh", "echo hello")])
    .env([("HOME", "/home/agent")])
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
    .build();
```

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

## Recipes

See [docs/recipes/](docs/recipes/) for detailed guides on common tasks:

- Embedding in an AI agent loop
- Seeding the filesystem from a real project
- Running in the browser with WASM
- Writing custom commands
- And more

## Documentation

- [Guidebook](docs/guidebook/) — internal engineering documentation (architecture, design, implementation details)
- [Recipes](docs/recipes/) — task-oriented guides for common use cases

## License

MIT
