# rust-bash

A sandboxed bash interpreter built in Rust. Execute bash scripts safely with a virtual filesystem — no containers, no VMs, no host access.

> ⚠️ **Status: Pre-alpha / Milestones 1–4 Complete** — Core interpreter, text processing,
> execution safety, and filesystem backends are implemented. Integration targets (C FFI, WASM,
> standalone CLI binary) are planned but not yet started.

## Highlights

- **Virtual filesystem** — all file operations happen in memory by default. No host files are touched.
- **80 commands** — echo, cat, grep, awk, sed, jq, find, sort, diff, curl, and many more.
- **Full bash syntax** — pipelines, redirections, variables, control flow, functions, command substitution, globs, brace expansion, arithmetic, here-documents, case statements.
- **Execution limits** — 10 configurable bounds (time, commands, loops, output size, call depth, string length, glob results, substitution depth, heredoc size, brace expansion).
- **Network policy** — sandboxed `curl` with URL allow-lists, method restrictions, redirect and response-size limits.
- **Multiple filesystem backends** — InMemoryFs (default), OverlayFs (copy-on-write), ReadWriteFs (passthrough), MountableFs (composite).
- **Embeddable** — use as a Rust crate with a builder API. Custom commands via the `VirtualCommand` trait.

## Quick Start

Add to `Cargo.toml`:

```toml
[dependencies]
rust-bash = { path = "..." }  # or a git/registry reference once published
```

### Basic usage

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

### Interactive REPL (example)

An interactive shell is provided as a runnable example:

```bash
cargo run --example shell

# Seed environment variables and files from a host directory
cargo run --example shell -- --env KEY=VAL --files ./seed-dir
```

## Use Cases

- **AI agent tools** — give LLMs a bash sandbox without container overhead
- **Code sandboxes** — run user-submitted scripts safely
- **Testing** — deterministic bash execution with a controlled filesystem
- **Embedded scripting** — add bash scripting to Rust applications

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

## Configuration

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

### Custom commands

Register domain-specific commands via the `VirtualCommand` trait:

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

## Public API

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

## Roadmap

The following are planned but not yet implemented (Milestone 5):

- Standalone CLI binary with `--files`, `--cwd`, `--env`, `--json` flags
- C FFI for embedding from Python/Go/Ruby
- WASM target for browser execution
- AI SDK integration (OpenAI/Anthropic tool definitions)

## License

MIT
