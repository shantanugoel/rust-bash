# Chapter 2: Architecture Overview

## Strategy: brush-parser + Custom Interpreter

We evaluated three approaches and chose **Strategy B+**:

| Strategy | Description | Verdict |
|----------|-------------|---------|
| A: Fork brush-core | Add VFS support to brush-core directly | Rejected — 225+ `std::fs` call sites, fork maintenance burden |
| **B+: Parser-only** ✅ | Use brush-parser for parsing + word expansion; custom interpreter + VFS | **Chosen** — clean separation, no fork, VFS-native from day one |
| C: Wrap brush-core | Intercept at command level | Rejected — can't intercept redirect setup without forking |

**Why B+ works**:
- `brush-parser` is standalone (no brush-core dependency), WASM-ready, MIT licensed
- `brush_parser::word::parse()` handles the hardest part — decomposing word strings into expansion pieces
- We get a full bash grammar parser for free; we only write the execution engine
- VFS is native from day one — no retrofitting real FS abstractions

## High-Level Architecture

```
┌─────────────────────────────────────────────────────┐
│                    Public API                        │
│  RustBash::builder().files(...).env(...).build()      │
│  shell.exec("cat file.txt | grep pattern")         │
└──────────────────┬──────────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────────┐
│              brush-parser                            │
│  tokenize_str() → parse_tokens() → Program (AST)    │
│  word::parse() → Vec<WordPiece> (expansion pieces)   │
└──────────────────┬──────────────────────────────────┘
                   │
┌──────────────────▼──────────────────────────────────┐
│            Interpreter Engine                        │
│  AST walker: compounds, pipelines, redirections      │
│  Word expansion: variables, quoting, globs, $()      │
│  Control flow: if/for/while/until/case/functions     │
│  Execution limits enforcement                        │
└───────┬─────────────────────┬───────────────────────┘
        │                     │
┌───────▼────────┐   ┌───────▼────────────────────────┐
│  Command        │   │  Virtual Filesystem (VFS)       │
│  Registry       │   │                                 │
│  70+ commands   │   │  trait VirtualFs                 │
│  dispatched by  │   │  ├── InMemoryFs (default)       │
│  name lookup    │   │  ├── OverlayFs (CoW over real)  │
│                 │   │  ├── ReadWriteFs (passthrough)   │
│  CommandContext  │   │  └── MountableFs (composites)   │
│  provides fs,   │   │                                 │
│  cwd, env,      │   │  All commands receive            │
│  stdin          │   │  &dyn VirtualFs, never &std::fs  │
└─────────────────┘   └─────────────────────────────────┘
```

## Module Structure

```
rust-bash/
├── src/
│   ├── lib.rs              # Module declarations, public re-exports
│   ├── api.rs              # Public API: RustBash, RustBashBuilder, ExecResult
│   ├── interpreter/
│   │   ├── mod.rs          # InterpreterState, top-level execute_program()
│   │   ├── expand.rs       # Word expansion (variables, quoting, globs, $())
│   │   ├── pipeline.rs     # Pipeline and redirection execution
│   │   └── control.rs      # Compound commands: if/for/while/case/functions
│   ├── vfs/
│   │   ├── mod.rs          # VirtualFs trait definition
│   │   ├── memory.rs       # InMemoryFs — default sandboxed backend
│   │   ├── overlay.rs      # OverlayFs — copy-on-write over real directory
│   │   ├── readwrite.rs    # ReadWriteFs — passthrough to real filesystem
│   │   └── mountable.rs    # MountableFs — composite mount points
│   ├── commands/
│   │   ├── mod.rs          # VirtualCommand trait, CommandContext, registry
│   │   ├── file_ops.rs     # cat, cp, mv, rm, ln, stat, tee, touch, chmod
│   │   ├── text.rs         # grep, sort, uniq, cut, head, tail, wc, tr, rev
│   │   ├── nav.rs          # ls, find, basename, dirname, realpath, tree
│   │   ├── awk.rs          # awk implementation
│   │   ├── sed.rs          # sed implementation
│   │   ├── jq.rs           # jq via jaq-core
│   │   ├── net.rs          # curl with network policy
│   │   └── util.rs         # echo, printf, date, sleep, seq, expr, env, test
│   ├── limits.rs           # ExecutionLimits, counters, timeout enforcement
│   ├── network.rs          # NetworkPolicy, URL allow-listing
│   ├── ffi.rs              # C FFI layer
│   └── error.rs            # Unified error types (RustBashError hierarchy)
├── examples/
│   └── basic.rs            # Usage demonstration
├── Cargo.toml
└── tests/
    ├── interpreter.rs      # Interpreter integration tests
    ├── vfs.rs              # VFS integration tests
    ├── commands.rs         # Command integration tests
    └── bash_compat.rs      # Bash compatibility test suite
```

## Data Flow

A call to `shell.exec("echo $HOME | wc -c")` flows through:

1. **RustBash** receives the command string, initializes fresh stdout/stderr buffers
2. **brush-parser** tokenizes and parses into a `Program` AST
3. **Interpreter** walks the AST:
   - Recognizes a pipeline of two commands
   - Executes `echo $HOME`: expands `$HOME` via word expansion, dispatches to Echo command
   - Pipes echo's stdout as stdin to `wc -c`: dispatches to Wc command
4. **Commands** read/write through the VFS and return `CommandResult` (stdout, stderr, exit_code)
5. **RustBash** collects final stdout, stderr, exit_code into `ExecResult` and returns it

## State Model

The `RustBash` owns a persistent `InterpreterState`. Each `exec()` call mutates this state — VFS contents, environment variables, current working directory, and function definitions all persist across calls. Only stdout/stderr buffers are fresh per call.

```
┌─ RustBash ─────────────────────────────────────────┐
│  InterpreterState (persistent across exec() calls)  │
│  ├── fs: Box<dyn VirtualFs> (persistent)            │
│  ├── env: HashMap<String, String> (persistent)      │
│  ├── cwd: String (persistent, updated by cd)        │
│  ├── functions: HashMap<String, FunctionDef>        │
│  ├── last_exit_code: i32 (updated per command)      │
│  ├── limits: ExecutionLimits (immutable config)     │
│  └── commands: HashMap<String, Box<dyn Command>>    │
│                                                     │
│  exec("cmd1") → mutates state, returns ExecResult   │
│  exec("cmd2") → sees cmd1's writes in fs and env    │
└─────────────────────────────────────────────────────┘
```

## Key Dependency: brush-parser

brush-parser is used as a library dependency (not forked). We use these APIs:

| API | Purpose |
|-----|---------|
| `tokenize_str(input)` | Tokenize raw command string |
| `parse_tokens(&tokens, &options)` | Parse tokens into `Program` AST |
| `word::parse(raw_word, &options)` | Decompose word string into `Vec<WordPieceWithSource>` |

**Stability risk**: brush-parser's AST types are public but not versioned with stability guarantees. Breaking changes require interpreter updates. This is an accepted risk — the benefit of reusing a full bash grammar parser outweighs the cost. We pin to a specific git revision for reproducibility.

## Error Philosophy

All public APIs return `Result<T, RustBashError>`. The error hierarchy:

- `RustBashError::Parse` — brush-parser failed to parse the input
- `RustBashError::Execution` — runtime error during script execution
- `RustBashError::LimitExceeded` — an execution limit was hit
- `RustBashError::Vfs` — filesystem operation failed
- `RustBashError::Network` — network policy violation or HTTP error
- `RustBashError::Timeout` — wall-clock execution time exceeded

Errors implement `std::error::Error` + `Display`. Command-level errors are reported via stderr and exit codes (matching bash behavior), not by propagating Rust errors — only truly exceptional conditions become `RustBashError`.
