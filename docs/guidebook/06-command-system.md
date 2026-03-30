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
    pub stdin_bytes: Option<&'a [u8]>,
    pub limits: &'a ExecutionLimits,
    pub network_policy: &'a NetworkPolicy,
    pub exec: Option<ExecCallback<'a>>,
}
```

> **Note on env type**: Commands see a flattened `String → String` view of the environment. The interpreter internally stores `Variable` structs with metadata (exported flag, readonly flag, etc.) and projects the string values into `CommandContext`.

> **Note on exec type**: `ExecCallback<'a>` is `&'a dyn Fn(&str) -> Result<CommandResult, RustBashError>`. Commands like `xargs` and `find -exec` use this to invoke sub-commands through the interpreter.

| Field | Purpose |
|-------|---------|
| `fs` | VFS for all file operations |
| `cwd` | Current working directory for resolving relative paths |
| `env` | Environment variables (read-only from command's perspective) |
| `stdin` | Input piped from the previous command in a pipeline |
| `stdin_bytes` | Binary input from pipeline (used by compression commands) |
| `limits` | Execution limits (commands should respect max_output_size) |
| `network_policy` | Network access policy (checked by `curl` before any HTTP request) |
| `exec` | Callback to execute sub-commands (used by `xargs`, `find -exec`, etc.) |

## CommandResult

```rust
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub stdout_bytes: Option<Vec<u8>>,
}
```

Commands return structured results. The interpreter handles piping stdout between pipeline stages and collecting stderr. Binary commands (compression/archiving) use `stdout_bytes` for byte-transparent output.

## CommandMeta and `--help` Support

Every command and builtin can provide declarative metadata via `CommandMeta`:

```rust
pub struct CommandMeta {
    pub name: &'static str,
    pub synopsis: &'static str,        // e.g., "grep [OPTIONS] PATTERN [FILE...]"
    pub description: &'static str,     // One-line summary
    pub options: &'static [(&'static str, &'static str)],  // ("-n", "print line numbers")
    pub supports_help_flag: bool,      // false for echo, true, false, test, [
    pub flags: &'static [FlagInfo],    // Detailed flag metadata with status
}
```

Commands expose metadata through the `VirtualCommand::meta()` method (default returns `None`). Builtins provide metadata through the `builtin_meta()` function in the builtins module.

### `--help` Dispatch

When a command receives `--help` as its **first** argument, the dispatch layer intercepts it **before** any command code runs:

1. Look up `CommandMeta` for the command name (builtins first, then registered commands).
2. If `meta.supports_help_flag == true` → print formatted help to stdout, exit 0.
3. If `meta.supports_help_flag == false` → fall through to normal dispatch.
4. If no metadata found → fall through to normal dispatch.

### Bash Compatibility Opt-Outs

Some commands set `supports_help_flag: false` to match bash behavior:

| Command | Behavior with `--help` |
|---------|----------------------|
| `echo` | Prints literal `--help` |
| `true` | Exits 0 silently |
| `false` | Exits 1 silently |
| `test` | Treats `--help` as string operand (truthy) |
| `[` | Treats `--help` as string operand (truthy) |

## Command Resolution Order

When the interpreter encounters a command name, it resolves in this order:

1. **`--help` interception** — if the first argument is `--help`, check metadata and potentially return help text (see above).
2. **Shell builtins** — `cd`, `export`, `exit`, `set`, `local`, `return`, etc. Handled directly by the interpreter.
3. **User-defined functions** — stored in `InterpreterState.functions`.
4. **Registered commands** — looked up in `InterpreterState.commands` HashMap.
5. **"Command not found"** — stderr error, exit code 127.

External process execution is impossible by design — there is no fallback to `std::process::Command`.

## `which` Command — PATH-Based Resolution

The `which` command resolves command names using actual VFS-based PATH lookup, matching real bash behavior:

1. **Check builtins** — if the name is a shell builtin (via `is_builtin()`), output `{name}: shell built-in command`
2. **Search PATH** — split `$PATH` on `:`, check each directory in the VFS for a matching file
3. **Return first hit** — output the full path (e.g., `/bin/ls`)
4. **Exit 1** if not found

This works because `RustBashBuilder::build()` creates stub files in `/bin/` for every registered command and builtin. The stub files contain `#!/bin/bash\n# built-in: <name>`, making them visible to `ls /bin`, `test -f /bin/ls`, and PATH-based resolution.

## Default Environment Variables

The builder sets sensible defaults for variables not already provided by the caller:

| Variable | Default | Notes |
|----------|---------|-------|
| `PATH` | `/usr/bin:/bin` | |
| `HOME` | `/home/user` | |
| `USER` | `user` | |
| `PWD` | CWD value | |
| `OLDPWD` | (empty) | |
| `SHELL` | `/bin/bash` | |
| `BASH` | `/bin/bash` | |
| `BASH_VERSION` | crate version | |
| `HOSTNAME` | `rust-bash` | |
| `OSTYPE` | `linux-gnu` | |
| `TERM` | `xterm-256color` | |

Caller-provided env vars via `.env()` always take precedence — defaults are only set for keys not already present.

## Command Categories

### Shell Builtins (Interpreter-Handled)

These commands modify interpreter state and cannot be implemented as `VirtualCommand`:

`cd`, `export`, `unset`, `set`, `shift`, `local`, `declare`, `readonly`, `return`, `exit`, `break`, `continue`, `eval`, `source`/`.`, `read`, `trap`, `let`, `:`, `shopt`, `type`, `command`, `builtin`, `getopts`, `mapfile`/`readarray`, `pushd`, `popd`, `dirs`, `hash`, `wait`, `alias`, `unalias`, `printf`, `exec`, `sh`/`bash`, `help`, `history`

> `true` and `false` are **not** builtins — they are registered `VirtualCommand` implementations and resolved at step 3 (registered commands).

### File Operations

Commands that interact with the VFS for file I/O:

| Command | Key Flags | Notes |
|---------|-----------|-------|
| `cat` | `-n` (line numbers) | Concatenate files or stdin |
| `ls` | `-l`, `-a`, `-R`, `-1` | List directory entries |
| `mkdir` | `-p` | Create directories |
| `cp` | `-r` (recursive) | Copy via VFS |
| `mv` | | Rename via VFS |
| `rm` | `-r`, `-f` | Remove via VFS |
| `ln` | `-s` (symbolic) | Create links in VFS |
| `touch` | | Create file or update mtime |
| `stat` | | Display file metadata |
| `tee` | `-a` (append) | Write stdin to file and stdout |
| `chmod` | | Change file permissions in VFS |
| `tree` | | Display directory tree |
| `readlink` | `-f`, `-e`, `-m` | Resolve symlinks |
| `rmdir` | `-p` | Remove empty directories |
| `du` | `-s`, `-h`, `-a`, `-d` | Estimate file space usage |
| `split` | `-l`, `-b` | Split file into chunks |

### Text Processing

Commands that operate on string data (stdin or file contents):

| Command | Key Flags | Notes |
|---------|-----------|-------|
| `grep` | `-E`, `-i`, `-n`, `-r`, `-o`, `-v`, `-l`, `-c`, `-A`/`-B`/`-C` | Regex support via `regex` crate |
| `egrep` / `fgrep` | | Aliases for `grep -E` / `grep -F` |
| `rg` | `-i`, `-t`, `-T`, `-g`, `--vimgrep` | Ripgrep-compatible recursive search |
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
| `od` | `-A`, `-t` | Octal/hex/decimal dump |
| `tac` | | Reverse file line order |
| `comm` | `-1`, `-2`, `-3` | Compare sorted files |
| `join` | `-t`, `-j` | Join sorted files on field |
| `fmt` | `-w` | Reformat paragraph text |
| `column` | `-t`, `-s` | Columnate lists |
| `expand` | `-t` | Convert tabs to spaces |
| `unexpand` | `-a`, `-t` | Convert spaces to tabs |
| `diff` | `-u` | Compare files |
| `strings` | `-n` | Extract printable strings from binary data |

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
| `which` | Show command path via PATH-based VFS resolution |
| `xargs` | Build and execute commands from stdin |
| `test` / `[` | Conditional expressions |
| `base64` | Encode/decode |
| `md5sum` / `sha1sum` / `sha256sum` | Hash computation |
| `whoami` / `hostname` / `uname` | Return sandbox-configured values |
| `yes` | Repeat output (with iteration limit!) |
| `timeout` | Run command with time limit |
| `file` | Detect file type via magic bytes |
| `bc` | Arbitrary precision calculator |
| `clear` | Output ANSI clear-screen escape sequence |

### Compression and Archiving

Commands that compress, decompress, and archive binary data. These use `stdout_bytes`/`stdin_bytes`
for byte-transparent pipeline propagation (binary data is never corrupted by UTF-8 conversion).

| Command | Key Flags | Notes |
|---------|-----------|-------|
| `gzip` | `-d`, `-c`, `-k`, `-f`, `-1`..`-9` | Compress files via `flate2` crate |
| `gunzip` | `-c`, `-k`, `-f` | Decompress (equivalent to `gzip -d`) |
| `zcat` | | Decompress to stdout (equivalent to `gzip -dc`) |
| `tar` | `-c`, `-x`, `-t`, `-f`, `-z`, `-v`, `-C` | Create, extract, list archives. `-z` for gzip compression |

### Network

| Command | Notes |
|---------|-------|
| `curl` | Sandboxed HTTP — validates every request against `NetworkPolicy` |

## Implementing a New Command

To add a command:

1. Create a struct implementing `VirtualCommand`
2. Implement `name()` → the command name string
3. Implement `meta()` → return a `&'static CommandMeta` with help text and options
4. Implement `execute()` → parse args, read from `ctx.fs`/`ctx.stdin`, return `CommandResult`
5. Register in the default command registry

```rust
use crate::commands::{CommandContext, CommandMeta, CommandResult, VirtualCommand};

pub struct MyCommand;

static MY_COMMAND_META: CommandMeta = CommandMeta {
    name: "mycommand",
    synopsis: "mycommand [OPTIONS] [ARG ...]",
    description: "Do something useful.",
    options: &[
        ("-v", "verbose output"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl VirtualCommand for MyCommand {
    fn name(&self) -> &str { "mycommand" }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&MY_COMMAND_META)
    }

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

> **Note**: The `--help` flag is handled automatically by the dispatch layer. You do not need to check for `--help` in your `execute()` method — just provide the `CommandMeta`.

## Custom Commands (User-Provided)

The `RustBashBuilder` allows registering custom commands:

```rust
let mut shell = RustBashBuilder::new()
    .command(Box::new(MyCustomCommand))
    .build()
    .unwrap();
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

### `unknown_option()` Helper

The `unknown_option(cmd, option)` helper in `src/commands/mod.rs` produces standardized error messages for unrecognized flags, matching bash/GNU conventions:

- Long options (`--foo`): `cmd: unrecognized option '--foo'`
- Short options (`-x`): `cmd: invalid option -- 'x'`

Both return exit code 2. Commands should call this instead of crafting ad-hoc error messages:

```rust
use crate::commands::unknown_option;

// In a command's flag parsing:
_ => return unknown_option("mycommand", arg),
```

## FlagInfo and FlagStatus Metadata

`CommandMeta` includes an optional `flags` field for introspection of per-command flag support status:

```rust
pub enum FlagStatus {
    Supported,  // Fully implemented
    Stubbed,    // Accepted but incomplete
    Ignored,    // Recognized but silently ignored
}

pub struct FlagInfo {
    pub flag: &'static str,       // e.g. "-n" or "--number"
    pub description: &'static str,
    pub status: FlagStatus,
}
```

Commands declare their flag metadata in a static array referenced by `CommandMeta::flags`. When `flags` is non-empty, `format_help()` appends a "Flag support" section to the `--help` output showing each flag's status.

Default `flags` to `&[]` for commands that haven't been annotated yet — this is backward-compatible and doesn't affect existing behavior.
