# AGENTS.md — rust-bash for AI Agents

> Quick-start guide for AI agents consuming the `rust-bash` npm package.
> For contributing to rust-bash itself, see the repo root [AGENTS.md](../../AGENTS.md).

## What is rust-bash?

A **sandboxed bash interpreter** written in Rust, exposed as an npm package.
Every command runs in-process against an **in-memory virtual filesystem** — no
real files are touched, no child processes are spawned, no network calls are
made (unless explicitly enabled). This makes it safe for untrusted or
AI-generated shell scripts.

**Key properties:**

- 79 built-in commands + 40 shell builtins (no external binaries)
- Full bash syntax: pipelines, redirections, variables, loops, functions, globs, arithmetic, heredocs, arrays
- Deterministic — no ambient host state leaks in
- Configurable execution limits (time, output size, loop iterations, call depth)

## Installation

```bash
npm install rust-bash
```

## Quick Start

### TypeScript

```typescript
import { Bash, tryLoadNative, createNativeBackend, initWasm, createWasmBackend } from 'rust-bash';

// Pick the fastest available backend
let createBackend;
if (await tryLoadNative()) {
  createBackend = createNativeBackend;
} else {
  await initWasm();
  createBackend = createWasmBackend;
}

// Create a sandboxed shell with pre-seeded files
const bash = await Bash.create(createBackend, {
  files: {
    '/data.json': '{"users": [{"name": "Alice"}, {"name": "Bob"}]}',
  },
  env: { USER: 'agent' },
});

// Execute commands — just like real bash
const result = await bash.exec('jq -r ".users[].name" /data.json | sort');
console.log(result.stdout);   // "Alice\nBob\n"
console.log(result.exitCode); // 0
```

### Seed files, run a pipeline, read the result

```typescript
const bash = await Bash.create(createBackend, {
  files: {
    '/input.csv': 'name,age\nAlice,30\nBob,25\nCharlie,35',
  },
});

// Multi-step pipeline
await bash.exec('tail -n +2 /input.csv | sort -t, -k2 -n | head -1 > /youngest.txt');
const youngest = bash.fs.readFileSync('/youngest.txt'); // "Bob,25\n"
```

## Available Commands (79)

### File System (17)

| Command | Description |
|---------|-------------|
| `cat` | Concatenate and display files |
| `cp` | Copy files and directories |
| `chmod` | Change file permissions |
| `du` | Estimate file/directory disk usage |
| `find` | Search for files by name, type, size, etc. |
| `ln` | Create hard and symbolic links |
| `ls` | List directory contents |
| `mkdir` | Create directories (`-p` for recursive) |
| `mv` | Move/rename files |
| `readlink` | Print resolved symbolic link target |
| `rm` | Remove files and directories |
| `rmdir` | Remove empty directories |
| `split` | Split files into pieces |
| `stat` | Display file metadata |
| `tee` | Copy stdin to file(s) and stdout |
| `touch` | Create files or update timestamps |
| `tree` | Display directory tree |

### Text Processing (29)

| Command | Description |
|---------|-------------|
| `awk` | Pattern scanning and processing |
| `column` | Columnate lists |
| `comm` | Compare two sorted files line by line |
| `cut` | Extract fields/columns from lines |
| `diff` | Compare files line by line |
| `egrep` | Extended regex grep |
| `expand` | Convert tabs to spaces |
| `fgrep` | Fixed-string grep |
| `fmt` | Reformat paragraph text |
| `fold` | Wrap lines to specified width |
| `grep` | Search text with patterns (`-E`, `-P`, `-r`, `-c`, `-l`, etc.) |
| `head` | Output first N lines |
| `join` | Join lines of two files on a common field |
| `jq` | JSON processor (jaq engine — full jq syntax) |
| `nl` | Number lines |
| `od` | Octal/hex dump |
| `paste` | Merge lines of files side by side |
| `printf` | Formatted output |
| `rev` | Reverse lines character by character |
| `rg` | Ripgrep — fast recursive search |
| `sed` | Stream editor for text transformation |
| `sort` | Sort lines (`-n`, `-r`, `-k`, `-t`, `-u`, etc.) |
| `strings` | Extract printable strings from binary data |
| `tac` | Reverse file line by line |
| `tail` | Output last N lines (`-f` not supported) |
| `tr` | Translate/delete characters |
| `unexpand` | Convert spaces to tabs |
| `uniq` | Report or omit repeated lines |
| `wc` | Count lines, words, bytes |

### Compression & Archiving (4)

| Command | Description |
|---------|-------------|
| `gzip` | Compress files (gzip format) |
| `gunzip` | Decompress gzip files |
| `tar` | Create/extract tar archives (`-c`, `-x`, `-t`, `-z` for gzip) |
| `zcat` | Display compressed file contents |

### Path Utilities (3)

| Command | Description |
|---------|-------------|
| `basename` | Strip directory from path |
| `dirname` | Extract directory from path |
| `realpath` | Resolve path to absolute |

### System & Environment (10)

| Command | Description |
|---------|-------------|
| `date` | Display/format date and time |
| `env` | Display or set environment for a command |
| `file` | Determine file type |
| `hostname` | Print hostname |
| `printenv` | Print environment variables |
| `sleep` | Pause execution for N seconds |
| `timeout` | Run command with time limit |
| `uname` | Print system information |
| `which` | Locate a command |
| `whoami` | Print current user name |

### Data Processing & Math (7)

| Command | Description |
|---------|-------------|
| `base64` | Encode/decode base64 |
| `bc` | Arbitrary precision calculator |
| `expr` | Evaluate expressions |
| `md5sum` | Compute MD5 hash |
| `seq` | Print number sequences |
| `sha1sum` | Compute SHA-1 hash |
| `sha256sum` | Compute SHA-256 hash |

### Core (9)

| Command | Description |
|---------|-------------|
| `clear` | Clear terminal screen |
| `echo` | Print arguments to stdout |
| `false` | Return exit code 1 |
| `pwd` | Print working directory |
| `test` / `[` | Evaluate conditional expressions |
| `true` | Return exit code 0 |
| `xargs` | Build and execute commands from stdin |
| `yes` | Repeatedly output a string |

### Network (native backend only, disabled by default)

| Command | Description |
|---------|-------------|
| `curl` | HTTP client (requires `network.enabled: true`) |

> **Note:** `curl` is only available with the **native** backend. It is **not available in WASM** because the WASM sandbox cannot make real HTTP requests. If you need network access, use `tryLoadNative()` / `createNativeBackend`.

## Shell Builtins (40)

These are handled by the interpreter, not as external commands:

| Builtin | Description |
|---------|-------------|
| `exit` | Exit the shell with a status code |
| `cd` | Change working directory |
| `export` | Set environment variable |
| `unset` | Remove variable or function |
| `set` / `shopt` | Set/query shell options |
| `shift` | Shift positional parameters |
| `readonly` / `declare` | Declare variables with attributes |
| `read` | Read line from stdin into variables |
| `eval` | Evaluate string as shell command |
| `source` / `.` | Execute file in current shell |
| `break` / `continue` | Loop control |
| `:` / `colon` | No-op (always returns 0) |
| `let` | Arithmetic evaluation |
| `local` | Declare local variable in function |
| `return` | Return from function |
| `trap` | Set signal/exit handlers |
| `type` | Describe how a name would be interpreted |
| `command` | Run command bypassing functions |
| `builtin` | Run a shell builtin explicitly |
| `getopts` | Parse positional parameters |
| `mapfile` / `readarray` | Read lines into array |
| `pushd` / `popd` / `dirs` | Directory stack operations |
| `hash` | Manage command hash table |
| `wait` | Wait for background jobs |
| `alias` / `unalias` | Create/remove command aliases |
| `printf` | Formatted output (builtin version) |
| `exec` | Replace shell with command |
| `sh` / `bash` | Execute a script |
| `help` | Display builtin help |
| `history` | Display command history |

## Default Environment

When you create a shell instance without overriding env, these defaults are set:

| Variable | Default | Description |
|----------|---------|-------------|
| `PATH` | `/usr/bin:/bin` | Command search path |
| `HOME` | `/home/user` | Home directory |
| `USER` | `user` | Current username |
| `HOSTNAME` | `rust-bash` | Host name |
| `SHELL` | `/bin/bash` | Shell path |
| `BASH` | `/bin/bash` | Bash binary path |
| `BASH_VERSION` | *(crate version)* | Version string |
| `OSTYPE` | `linux-gnu` | OS type identifier |
| `TERM` | `xterm-256color` | Terminal type |
| `OLDPWD` | `""` | Previous working directory |
| `PWD` | *(cwd)* | Current directory |

### Default Virtual Filesystem Layout

```
/bin/                  # Standard command location
/usr/bin/              # Alternative command location
/tmp/                  # Temporary files
/dev/null              # Write sink (discards input)
/dev/zero              # Zero-byte source
/dev/stdin             # Standard input device
/dev/stdout            # Standard output device
/dev/stderr            # Standard error device
/home/user/            # User home directory
```

Files you seed via `files: { ... }` in the options are written **before** defaults, so your files are never overwritten.

## Sandboxed Behavior

**What works like real bash:**

- Pipelines, redirections, subshells, command substitution
- Variables, arrays, associative arrays, arithmetic
- `if`/`else`, `for`, `while`, `until`, `case`, `select`
- Functions, local variables, `return`
- Globs (`*`, `?`, `**`, `{a,b}`), tilde expansion
- Heredocs, herestrings
- `trap` for ERR/EXIT/DEBUG handlers
- `set -e`, `set -o pipefail`, `set -u`

**What is different:**

- **No real filesystem** — all file I/O uses the in-memory VirtualFs
- **No child processes** — commands like `xargs`, `find -exec` call back into the interpreter
- **No networking by default** — `curl` requires explicit `network: { enabled: true, ... }` (native backend only; unavailable in WASM)
- **No job control** — no `&` background, no `fg`/`bg`/`jobs`
- **No signals** — `kill` is not implemented; `trap` handles EXIT/ERR/DEBUG only
- **`sleep` is instant** — returns immediately (no real delay)
- **`date` is deterministic** — returns a fixed timestamp unless overridden
- **No `/proc`, `/sys`** — system pseudo-filesystems are not mounted
- **Binary pipeline support** — gzip/tar can pipe binary data between commands

## Common Patterns for AI Agents

### JSON processing

```bash
# Extract fields from JSON
echo '{"name":"Alice","age":30}' | jq '.name'

# Transform JSON arrays
cat /data.json | jq '[.items[] | {id: .id, label: .name}]'

# Process JSONL (one object per line)
cat /logs.jsonl | jq -r 'select(.level == "error") | .message'
```

### CSV processing

```bash
# Extract column 2 from CSV
cut -d, -f2 /data.csv

# Sort CSV by numeric third column
tail -n +2 /data.csv | sort -t, -k3 -n

# Count unique values in column 1
cut -d, -f1 /data.csv | sort | uniq -c | sort -rn
```

### File manipulation

```bash
# Find files by pattern
find / -name "*.json" -type f

# Batch rename via loop
for f in $(find /data -name "*.txt"); do
  mv "$f" "${f%.txt}.md"
done

# Compare two files
diff /expected.txt /actual.txt
```

### Text search and extraction

```bash
# Recursive grep with context
grep -rn "TODO" /src --include="*.rs" -A 2

# Extract matching lines and transform
grep -E "^ERROR" /app.log | sed 's/ERROR: //' | sort -u

# Count pattern occurrences per file
grep -rc "import" /src --include="*.ts" | sort -t: -k2 -rn
```

### Data pipelines

```bash
# Multi-step pipeline: filter → transform → aggregate
cat /access.log \
  | grep "POST /api" \
  | awk '{print $1}' \
  | sort | uniq -c | sort -rn \
  | head -10

# Compute hash of transformed data
cat /input.txt | tr '[:upper:]' '[:lower:]' | sort -u | sha256sum
```

### Compression

```bash
# Create a gzipped tarball
tar czf /archive.tar.gz -C /project .

# Extract tarball
tar xzf /archive.tar.gz -C /output

# Compress/decompress individual files
gzip /large-file.txt       # creates /large-file.txt.gz
gunzip /large-file.txt.gz  # restores /large-file.txt
```

### Using the VirtualFs API directly

```typescript
// Seed files before execution
const bash = await Bash.create(createBackend, {
  files: {
    '/config.json': JSON.stringify({ debug: true }),
    '/template.sh': 'echo "Hello, $NAME"',
  },
  env: { NAME: 'Agent' },
});

// Execute and read results
await bash.exec('bash /template.sh > /output.txt');
const output = bash.fs.readFileSync('/output.txt'); // "Hello, Agent\n"

// Read generated files after execution
await bash.exec('find / -name "*.log" -type f > /manifest.txt');
const manifest = bash.fs.readFileSync('/manifest.txt');
```

### Execution limits

```typescript
const bash = await Bash.create(createBackend, {
  executionLimits: {
    maxCommandCount: 1000,       // max commands per exec()
    maxExecutionTimeSecs: 5,     // wall-clock timeout
    maxLoopIterations: 1000,     // prevent infinite loops
    maxOutputSize: 1_048_576,    // 1 MB output cap
    maxCallDepth: 50,            // recursion limit
  },
});
```

### Network (native backend only)

```typescript
const bash = await Bash.create(createNativeBackend, {
  network: {
    enabled: true,
    allowedUrlPrefixes: ['https://api.example.com/'],
  },
});
```

> **Tip:** To allow all URLs, set `allowedUrlPrefixes` to `['http://', 'https://']`.
> Wildcards are not supported — the policy uses prefix matching.

## Limitations

- **No interactive commands** — `vi`, `less`, `nano`, etc. are not available
- **No package managers** — `apt`, `pip`, `npm` are not present
- **No `tail -f`** — follow mode is not supported (no real-time file watching)
- **No process management** — no `ps`, `kill`, `bg`, `fg`, `jobs`
- **No user/group management** — `useradd`, `chown`, `chgrp` are not available
- **Network is opt-in** — `curl` only works with the native backend when network is explicitly enabled with URL allow-list; not available in WASM
- **Binary tools are limited** — no `xxd`, `hexdump` (use `od` instead)
- **No YAML support** — `yq` is planned for a future milestone; use `grep`/`sed` for basic YAML extraction
- **No `sudo`** — the sandbox runs as a single virtual user

## Help flag

All commands support `--help` for usage information:

```bash
grep --help
jq --help
tar --help
```

## Links

- **npm**: [rust-bash](https://www.npmjs.com/package/rust-bash)
- **Repository**: [github.com/shantanugoel/rust-bash](https://github.com/shantanugoel/rust-bash)
- **Homepage**: [rustbash.dev](https://rustbash.dev)
