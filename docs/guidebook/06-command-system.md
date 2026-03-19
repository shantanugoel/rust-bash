# Chapter 6: Command System

## Overview

Commands are the units of work in rust-bash. Every executable name (`echo`, `grep`, `cat`, etc.) is resolved to a Rust implementation that receives structured inputs and produces structured outputs. No command ever spawns a real process.

## The VirtualCommand Trait

```rust
pub trait VirtualCommand: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult;
}
```

Commands are stateless — all context is provided through `CommandContext`. This makes them easy to test in isolation and safe to share across threads.

## CommandContext

```rust
pub struct CommandContext<'a> {
    pub fs: &'a dyn VirtualFs,
    pub cwd: &'a str,
    pub env: &'a HashMap<String, String>,
    pub stdin: &'a str,
    pub limits: &'a ExecutionLimits,
    pub exec: Option<&'a dyn Fn(&str) -> CommandResult>,
}
```

> **Note on env type**: Commands see a flattened `String → String` view of the environment. The interpreter internally stores `Variable` structs with metadata (exported flag, readonly flag, etc.) and projects the string values into `CommandContext`.

| Field | Purpose |
|-------|---------|
| `fs` | VFS for all file operations |
| `cwd` | Current working directory for resolving relative paths |
| `env` | Environment variables (read-only from command's perspective) |
| `stdin` | Input piped from the previous command in a pipeline |
| `limits` | Execution limits (commands should respect max_output_size) |
| `exec` | Callback to execute sub-commands (used by `xargs`, `find -exec`, etc.) |

## CommandResult

```rust
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
```

Commands return structured results. The interpreter handles piping stdout between pipeline stages and collecting stderr.

## Command Resolution Order

When the interpreter encounters a command name, it resolves in this order:

1. **Shell builtins** — `cd`, `export`, `exit`, `set`, `local`, `return`, etc. Handled directly by the interpreter.
2. **User-defined functions** — stored in `InterpreterState.functions`.
3. **Registered commands** — looked up in `InterpreterState.commands` HashMap.
4. **"Command not found"** — stderr error, exit code 127.

External process execution is impossible by design — there is no fallback to `std::process::Command`.

## Command Categories

### Shell Builtins (Interpreter-Handled)

These commands modify interpreter state and cannot be implemented as `VirtualCommand`:

`cd`, `export`, `unset`, `set`, `shift`, `local`, `declare`, `readonly`, `return`, `exit`, `break`, `continue`, `eval`, `source`/`.`, `read`, `trap`

> `true` and `false` are also handled as builtins for performance, though they don't modify state and could be regular commands.

### File Operations

Commands that interact with the VFS for file I/O:

| Command | Key Flags | Notes |
|---------|-----------|-------|
| `cat` | `-n` (line numbers) | Concatenate files or stdin |
| `cp` | `-r` (recursive) | Copy via VFS |
| `mv` | | Rename via VFS |
| `rm` | `-r`, `-f` | Remove via VFS |
| `ln` | `-s` (symbolic) | Create links in VFS |
| `touch` | | Create file or update mtime |
| `stat` | | Display file metadata |
| `tee` | `-a` (append) | Write stdin to file and stdout |
| `chmod` | | Change file permissions in VFS |
| `tree` | | Display directory tree |

### Text Processing

Commands that operate on string data (stdin or file contents):

| Command | Key Flags | Notes |
|---------|-----------|-------|
| `grep` | `-E`, `-i`, `-n`, `-r`, `-o`, `-v`, `-l`, `-c`, `-A`/`-B`/`-C` | Regex support via `regex` crate |
| `sort` | `-r`, `-n`, `-k`, `-t`, `-u` | Sort lines |
| `uniq` | `-c`, `-d`, `-u` | Deduplicate adjacent lines |
| `cut` | `-d`, `-f`, `-c` | Extract fields/columns |
| `head` | `-n` | First N lines |
| `tail` | `-n` | Last N lines |
| `wc` | `-l`, `-w`, `-c` | Count lines/words/bytes |
| `tr` | `-d`, `-s` | Translate/delete characters |
| `rev` | | Reverse lines |
| `fold` | `-w`, `-s` | Wrap lines at width |
| `nl` | | Number lines |
| `paste` | `-d` | Merge lines of files |
| `comm` | `-1`, `-2`, `-3` | Compare sorted files |
| `join` | `-t`, `-j` | Join sorted files on field |
| `diff` | `-u` | Compare files |

### Mini-Languages

Sub-interpreters for domain-specific languages:

| Command | Implementation Approach |
|---------|----------------------|
| `awk` | Custom interpreter — field splitting, patterns, actions, built-in functions |
| `sed` | Custom interpreter — address matching, s///, hold space |
| `jq` | Via `jaq-core` crate — battle-tested jq implementation in Rust |

These are the most complex commands. Each is effectively a mini-programming-language. See the implementation plan (Chapter 10) for scoping decisions on the 80/20 subset.

### Navigation

Commands that traverse the VFS directory structure:

| Command | Key Flags | Notes |
|---------|-----------|-------|
| `ls` | `-l`, `-a`, `-R`, `-1` | List directory contents |
| `find` | `-name`, `-type`, `-maxdepth` | Search directory tree |
| `basename` | | Strip directory from path |
| `dirname` | | Strip last component from path |
| `realpath` | | Resolve path via VFS canonicalize |
| `pwd` | | Print working directory |

### Utilities

Pure computation or environment lookups:

| Command | Notes |
|---------|-------|
| `echo` | Print arguments |
| `printf` | Formatted output |
| `date` | Date/time formatting (uses real or injected clock) |
| `sleep` | Pause execution (respects timeout limits) |
| `seq` | Generate number sequences |
| `expr` | Evaluate expressions |
| `env` / `printenv` | Display environment |
| `which` | Show command path (always "builtin" for registered commands) |
| `xargs` | Build and execute commands from stdin |
| `test` / `[` | Conditional expressions |
| `base64` | Encode/decode |
| `md5sum` / `sha256sum` | Hash computation |
| `whoami` / `hostname` / `uname` | Return sandbox-configured values |
| `yes` | Repeat output (with iteration limit!) |

### Network

| Command | Notes |
|---------|-------|
| `curl` | Sandboxed HTTP — validates every request against `NetworkPolicy` |

## Implementing a New Command

To add a command:

1. Create a struct implementing `VirtualCommand`
2. Implement `name()` → the command name string
3. Implement `execute()` → parse args, read from `ctx.fs`/`ctx.stdin`, return `CommandResult`
4. Register in the default command registry

```rust
pub struct MyCommand;

impl VirtualCommand for MyCommand {
    fn name(&self) -> &str { "mycommand" }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        // Parse arguments
        // Do work using ctx.fs, ctx.stdin, ctx.env
        // Return result
        CommandResult {
            stdout: "output\n".to_string(),
            stderr: String::new(),
            exit_code: 0,
        }
    }
}
```

## Custom Commands (User-Provided)

The `RustBashBuilder` allows registering custom commands:

```rust
let mut shell = RustBash::builder()
    .command(Box::new(MyCustomCommand))
    .build();
```

Custom commands have the same capabilities as built-in commands — full VFS access, environment access, and stdin. This is the extension point for domain-specific tools.

## Argument Parsing Conventions

Commands should follow GNU/POSIX argument conventions:
- Short flags: `-n`, `-r`, `-v`
- Long flags: `--recursive`, `--verbose`
- `--` terminates flag parsing
- Combined short flags: `-rn` equivalent to `-r -n`

For commands with complex argument parsing, consider using a lightweight argument parser. For simple commands, manual parsing is fine.

## Error Conventions

Commands report errors via stderr and exit code, not by returning Rust errors:
- Exit code 0: success
- Exit code 1: general error
- Exit code 2: misuse of command (bad arguments)
- Exit code 127: command not found (set by interpreter, not commands)

Error messages should follow the format: `command_name: message` (e.g., `cat: /nonexistent: No such file or directory`).
