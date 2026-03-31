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
│  RustBashBuilder::new().files(...).env(...).build()   │
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
│   ├── api.rs              # Public API: RustBash, RustBashBuilder
│   ├── interpreter/
│   │   ├── mod.rs          # InterpreterState, ExecutionLimits, parse(), core types
│   │   ├── walker.rs       # AST walking: pipelines, redirections, compound commands
│   │   ├── expansion.rs    # Word expansion (variables, quoting, globs, $())
│   │   ├── arithmetic.rs   # Arithmetic expression evaluator ($((…)), let, ((…)))
│   │   ├── brace.rs        # Brace expansion ({a,b,c}, {1..10..2})
│   │   ├── builtins.rs     # Shell builtins (cd, export, set, trap, local, …)
│   │   └── pattern.rs      # Glob pattern matching for case/pathname expansion
│   ├── vfs/
│   │   ├── mod.rs          # VirtualFs trait definition, Metadata, FsNode types
│   │   ├── memory.rs       # InMemoryFs — default sandboxed backend
│   │   ├── overlay.rs      # OverlayFs — copy-on-write over real directory
│   │   ├── readwrite.rs    # ReadWriteFs — passthrough to real filesystem
│   │   └── mountable.rs    # MountableFs — composite mount points
│   ├── commands/
│   │   ├── mod.rs          # VirtualCommand trait, CommandContext, echo, registry
│   │   ├── file_ops.rs     # cp, mv, rm, tee, stat, chmod, ln
│   │   ├── text.rs         # grep, sort, uniq, cut, head, tail, wc, tr, rev, fold,
│   │   │                   # nl, printf, paste, tac, comm, join, fmt, column,
│   │   │                   # expand, unexpand
│   │   ├── navigation.rs   # realpath, basename, dirname, tree
│   │   ├── awk/            # Full awk implementation (lexer, parser, runtime)
│   │   │   ├── mod.rs
│   │   │   ├── lexer.rs
│   │   │   ├── parser.rs
│   │   │   └── runtime.rs
│   │   ├── sed.rs          # sed stream editor
│   │   ├── diff_cmd.rs     # diff (unified, context, normal formats)
│   │   ├── jq_cmd.rs       # jq via jaq-core
│   │   ├── exec_cmds.rs    # xargs, find
│   │   ├── test_cmd.rs     # test / [ command
│   │   ├── net.rs          # curl with network policy
│   │   ├── utils.rs        # expr, date, sleep, seq, env, printenv, which, base64,
│   │   │                   # md5sum, sha256sum, whoami, hostname, uname, yes
│   │   └── regex_util.rs   # BRE→ERE conversion shared by grep/sed/expr
│   ├── network.rs          # NetworkPolicy, URL allow-listing
│   └── error.rs            # Unified error types (RustBashError, VfsError)
├── examples/
│   └── shell.rs            # Interactive REPL shell
├── Cargo.toml
└── tests/
    ├── integration.rs          # End-to-end shell integration tests
    ├── filesystem_backends.rs  # VFS backend integration tests
    └── snapshots/              # insta snapshot files
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
┌─ RustBash ───────────────────────────────────────────────┐
│  InterpreterState (persistent across exec() calls)        │
│  ├── fs: Arc<dyn VirtualFs>        (VFS, persistent)      │
│  ├── env: HashMap<String, Variable> (persistent)          │
│  ├── cwd: String                   (updated by cd)        │
│  ├── functions: HashMap<String, FunctionDef>              │
│  ├── last_exit_code: i32           (updated per command)  │
│  ├── commands: HashMap<String, Arc<dyn VirtualCommand>>   │
│  ├── shell_opts: ShellOpts         (errexit, nounset, …)  │
│  ├── limits: ExecutionLimits       (immutable config)     │
│  ├── counters: ExecutionCounters   (reset per exec())     │
│  ├── network_policy: NetworkPolicy                        │
│  ├── traps: HashMap<String, String>                       │
│  ├── positional_params: Vec<String>                       │
│  └── (internal: loop_depth, control_flow, local_scopes,   │
│       in_function_depth, random_seed, …)                  │
│                                                           │
│  exec("cmd1") → mutates state, returns ExecResult         │
│  exec("cmd2") → sees cmd1's writes in fs and env          │
└───────────────────────────────────────────────────────────┘
```

## Key Dependency: brush-parser

brush-parser is used as a library dependency (not forked). We use these APIs:

| API | Purpose |
|-----|---------|
| `tokenize_str(input)` | Tokenize raw command string |
| `parse_tokens(&tokens, &options, &source_info)` | Parse tokens into `Program` AST |
| `word::parse(raw_word, &options)` | Decompose word string into `Vec<WordPieceWithSource>` |

**Stability risk**: brush-parser's AST types are public but not versioned with stability guarantees. Breaking changes require interpreter updates. This is an accepted risk — the benefit of reusing a full bash grammar parser outweighs the cost. We pin to a specific crates.io version (`brush-parser = "0.3.0"`) for reproducibility.

## Error Philosophy

All public APIs return `Result<T, RustBashError>`. The error hierarchy:

- `RustBashError::Parse` — brush-parser failed to parse the input
- `RustBashError::Execution` — runtime error during script execution
- `RustBashError::LimitExceeded` — an execution limit was hit
- `RustBashError::Vfs` — filesystem operation failed
- `RustBashError::Network` — network policy violation or HTTP error
- `RustBashError::Timeout` — wall-clock execution time exceeded

Errors implement `std::error::Error` + `Display`. Command-level errors are reported via stderr and exit codes (matching bash behavior), not by propagating Rust errors — only truly exceptional conditions become `RustBashError`.
