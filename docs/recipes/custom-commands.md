# Custom Commands

## Goal

Register domain-specific commands that scripts can call like any built-in. Custom commands have full access to the virtual filesystem, environment, and stdin.

## Basic Custom Command

Implement the `VirtualCommand` trait and register it via the builder:

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};

struct Greet;

impl VirtualCommand for Greet {
    fn name(&self) -> &str {
        "greet"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let name = args.first().map(|s| s.as_str()).unwrap_or("world");
        CommandResult {
            stdout: format!("Hello, {name}!\n"),
            stderr: String::new(),
            exit_code: 0,
        }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(Greet))
    .build()
    .unwrap();

let result = shell.exec("greet Alice").unwrap();
assert_eq!(result.stdout, "Hello, Alice!\n");

// Custom commands work in pipelines and redirections like any built-in
let result = shell.exec("greet Bob | tr '[:lower:]' '[:upper:]'").unwrap();
assert_eq!(result.stdout, "HELLO, BOB!\n");
```

## Using the CommandContext

The `CommandContext` gives your command access to the shell's resources:

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};
use std::path::Path;
use std::collections::HashMap;

struct FileInfo;

impl VirtualCommand for FileInfo {
    fn name(&self) -> &str {
        "fileinfo"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let path_str = match args.first() {
            Some(p) => p.as_str(),
            None => {
                return CommandResult {
                    stderr: "fileinfo: missing path argument\n".into(),
                    exit_code: 1,
                    ..Default::default()
                };
            }
        };

        // Resolve relative paths against cwd
        let path = if path_str.starts_with('/') {
            Path::new(path_str).to_path_buf()
        } else {
            Path::new(ctx.cwd).join(path_str)
        };

        // Read from the virtual filesystem
        match ctx.fs.read_file(&path) {
            Ok(content) => {
                let size = content.len();
                let lines = content.iter().filter(|&&b| b == b'\n').count();
                CommandResult {
                    stdout: format!("{path_str}: {size} bytes, {lines} lines\n"),
                    ..Default::default()
                }
            }
            Err(e) => CommandResult {
                stderr: format!("fileinfo: {e}\n"),
                exit_code: 1,
                ..Default::default()
            },
        }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(FileInfo))
    .files(HashMap::from([
        ("/data.txt".into(), b"line1\nline2\nline3\n".to_vec()),
    ]))
    .build()
    .unwrap();

let result = shell.exec("fileinfo /data.txt").unwrap();
assert_eq!(result.stdout, "/data.txt: 18 bytes, 3 lines\n");
```

### What's in CommandContext

| Field | Type | Description |
|-------|------|-------------|
| `fs` | `&dyn VirtualFs` | The virtual filesystem — read/write files, list directories |
| `cwd` | `&str` | Current working directory |
| `env` | `&HashMap<String, String>` | All shell variables (names → values) |
| `stdin` | `&str` | Standard input (piped data or redirect content) |
| `limits` | `&ExecutionLimits` | Current execution limits |
| `network_policy` | `&NetworkPolicy` | Network access policy |
| `exec` | `Option<ExecCallback>` | Callback for sub-command execution (used by `xargs`/`find -exec`) |

## Processing stdin

Custom commands receive piped input through `ctx.stdin`:

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};

struct WordCount;

impl VirtualCommand for WordCount {
    fn name(&self) -> &str {
        "mycount"
    }

    fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
        let words: usize = ctx.stdin.split_whitespace().count();
        CommandResult {
            stdout: format!("{words}\n"),
            ..Default::default()
        }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(WordCount))
    .build()
    .unwrap();

let result = shell.exec("echo 'one two three four' | mycount").unwrap();
assert_eq!(result.stdout, "4\n");
```

## Registering Multiple Commands

Chain `.command()` calls on the builder:

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};

struct CmdA;
impl VirtualCommand for CmdA {
    fn name(&self) -> &str { "cmd-a" }
    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult { stdout: "A\n".into(), ..Default::default() }
    }
}

struct CmdB;
impl VirtualCommand for CmdB {
    fn name(&self) -> &str { "cmd-b" }
    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult { stdout: "B\n".into(), ..Default::default() }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(CmdA))
    .command(Box::new(CmdB))
    .build()
    .unwrap();

let result = shell.exec("cmd-a && cmd-b").unwrap();
assert_eq!(result.stdout, "A\nB\n");
```

## Overriding Built-in Commands

If your custom command uses the same name as a built-in, it replaces the built-in:

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};

struct AuditedEcho;

impl VirtualCommand for AuditedEcho {
    fn name(&self) -> &str {
        "echo" // overrides the built-in echo
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let text = args.join(" ");
        CommandResult {
            stdout: format!("[AUDIT] {text}\n"),
            ..Default::default()
        }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(AuditedEcho))
    .build()
    .unwrap();

let result = shell.exec("echo hello").unwrap();
assert_eq!(result.stdout, "[AUDIT] hello\n");
```

---

## TypeScript: Custom Commands with defineCommand

The `rust-bash` npm package provides `defineCommand()` for creating custom commands in TypeScript:

### Basic Command

```typescript
import { Bash, defineCommand } from 'rust-bash';

const greet = defineCommand('greet', async (args, ctx) => {
  const name = args[0] ?? 'world';
  return { stdout: `Hello, ${name}!\n`, stderr: '', exitCode: 0 };
});

const bash = await Bash.create(createBackend, {
  customCommands: [greet],
});

const result = await bash.exec('greet Alice');
// result.stdout === "Hello, Alice!\n"
```

### Async Commands (e.g., HTTP fetch)

```typescript
import { defineCommand } from 'rust-bash';

const fetchCmd = defineCommand('fetch', async (args, ctx) => {
  const url = args[0];
  if (!url) {
    return { stdout: '', stderr: 'fetch: missing URL\n', exitCode: 1 };
  }
  const response = await globalThis.fetch(url);
  const text = await response.text();
  return { stdout: text, stderr: '', exitCode: response.ok ? 0 : 1 };
});
```

### Accessing the Filesystem

Custom commands receive a `CommandContext` with VFS access:

```typescript
const countLines = defineCommand('count-lines', async (args, ctx) => {
  const path = args[0];
  if (!path) {
    return { stdout: '', stderr: 'count-lines: missing path\n', exitCode: 1 };
  }
  try {
    const content = ctx.fs.readFileSync(path);
    const lines = content.split('\n').length;
    return { stdout: `${lines}\n`, stderr: '', exitCode: 0 };
  } catch {
    return { stdout: '', stderr: `count-lines: ${path}: No such file\n`, exitCode: 1 };
  }
});
```

### Using exec() for Sub-Commands

Custom commands can invoke other commands:

```typescript
const deploy = defineCommand('deploy', async (args, ctx) => {
  const result = await ctx.exec('cat /app/manifest.json | jq -r .version');
  return {
    stdout: `Deploying version ${result.stdout.trim()}...\n`,
    stderr: '',
    exitCode: 0,
  };
});
```

### Multiple Commands

```typescript
const bash = await Bash.create(createBackend, {
  customCommands: [greet, fetchCmd, countLines, deploy],
});

await bash.exec('greet Bob && count-lines /data.txt');
```
