# CLI Usage

Run commands, seed files, set environment variables, and use the interactive REPL — all from the command line.

## Quick Start

```bash
# Install
cargo install --path .

# Run a command
rust-bash -c 'echo hello world'
```

## Execution Modes

### Inline command with `-c`

```bash
rust-bash -c 'echo hello | wc -c'
```

### Script file with positional arguments

```bash
rust-bash script.sh arg1 arg2
```

Inside the script, `$0` is set to the script path, and `$1`, `$2`, etc. are the
positional arguments.

### Piping commands via stdin

```bash
echo 'echo hello' | rust-bash

cat script.sh | rust-bash
```

### Interactive REPL

When launched with no `-c`, no script file, and no piped stdin, `rust-bash`
starts an interactive REPL:

```bash
rust-bash
```

REPL features:

- **Colored prompt** — `rust-bash:{cwd}$ ` reflecting the current directory, green (exit 0) or red (non-zero last exit)
- **Tab completion** — completes built-in command names
- **Multi-line input** — incomplete constructs (e.g., `if true; then`) wait for more input
- **History** — persists across sessions in `~/.rust_bash_history`
- **Ctrl-C** — cancels the current input line
- **Ctrl-D** — exits the REPL with the last command's exit code
- **`exit [N]`** — exits with code N (default 0)

## Seeding Files from Disk

Use `--files` to load host files into the virtual filesystem.

### Single file mapping

Map a host file to a specific path inside the VFS:

```bash
rust-bash --files /path/to/data.txt:/data.txt -c 'cat /data.txt'
```

The format is `HOST_PATH:VFS_PATH`. The first `:` in the value acts as the
separator.

### Directory seeding

Recursively load a host directory into a VFS path:

```bash
rust-bash --files /path/to/dir:/app -c 'ls /app'
```

Or seed an entire directory at the VFS root:

```bash
rust-bash --files /path/to/dir -c 'ls /'
```

### Multiple file mappings

Chain multiple `--files` flags:

```bash
rust-bash \
  --files ./src:/app/src \
  --files ./config.json:/app/config.json \
  -c 'cat /app/config.json'
```

## Setting Environment Variables

Use `--env` to set variables (repeatable):

```bash
rust-bash --env USER=agent --env HOME=/home/agent -c 'echo $USER'
# agent
```

Default environment variables (`HOME=/home`, `USER=user`, `PWD=/`) can be
overridden with `--env`.

## Setting the Working Directory

Use `--cwd` to set the initial working directory:

```bash
rust-bash --cwd /app -c 'pwd'
# /app
```

The directory is created automatically in the VFS if it doesn't exist.

## JSON Output for Scripting

Use `--json` to get machine-readable output:

```bash
rust-bash --json -c 'echo hello'
# {"stdout":"hello\n","stderr":"","exit_code":0}
```

Parse with `jq`:

```bash
rust-bash --json -c 'echo hello; echo err >&2; exit 42' | jq -r '.stdout'
# hello

rust-bash --json -c 'echo hello' | jq '.exit_code'
# 0
```

> **Note:** `--json` is not supported in interactive REPL mode. If used without
> `-c` or a script file and stdin is a terminal, `rust-bash` exits with code 2.

## Combining Flags

A realistic example seeding project files, setting environment, and producing
JSON output:

```bash
rust-bash \
  --files ./project:/app \
  --env APP_ENV=test \
  --env DATABASE_URL=sqlite:///app/db.sqlite \
  --cwd /app \
  --json \
  -c '
    echo "Environment: $APP_ENV"
    ls /app
    cat /app/config.json | jq .name
  '
```

## Flag Reference

| Flag | Short | Description |
|------|-------|-------------|
| `--command` | `-c` | Execute a command string and exit |
| `--files` | | Seed VFS from host files/directories (`HOST:VFS` or `HOST_DIR`) |
| `--env` | | Set environment variables (`KEY=VALUE`, repeatable) |
| `--cwd` | | Set initial working directory (default: `/`) |
| `--json` | | Output results as JSON |

**Execution priority:** `-c` > script file > stdin > REPL.

## See Also

- [Getting Started](getting-started.md) — embedding rust-bash as a Rust library
- [Guidebook Chapter 8](../guidebook/08-integration-targets.md) — integration target reference
