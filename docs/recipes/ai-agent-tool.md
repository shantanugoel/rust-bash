# Embedding in an AI Agent

## Goal

Use rust-bash as a bash execution tool for LLM-powered agents. The shell provides a sandboxed environment where the AI can run commands, inspect files, and process data — without containers, VMs, or host filesystem access.

## Why rust-bash for AI Agents?

| Feature | rust-bash | Docker/VM | Host bash |
|---------|-----------|-----------|-----------|
| Startup time | Microseconds | Seconds | Microseconds |
| Isolation | Virtual FS, execution limits | Full OS-level | None |
| Memory footprint | KBs | MBs–GBs | N/A |
| Custom commands | VirtualCommand trait | Mount scripts | PATH |
| Network control | URL allow-list | Network policies | iptables |
| Reproducible FS | Yes (InMemoryFs) | Mostly | No |

## Basic Agent Setup

```rust
use rust_bash::{RustBashBuilder, RustBashError, ExecutionLimits, NetworkPolicy};
use std::collections::HashMap;
use std::time::Duration;

struct AgentShell {
    shell: rust_bash::RustBash,
}

impl AgentShell {
    fn new() -> Self {
        let shell = RustBashBuilder::new()
            .env(HashMap::from([
                ("HOME".into(), "/home/agent".into()),
                ("USER".into(), "agent".into()),
            ]))
            .cwd("/home/agent")
            .execution_limits(ExecutionLimits {
                max_command_count: 5_000,
                max_execution_time: Duration::from_secs(10),
                max_output_size: 512 * 1024, // 512 KB
                ..Default::default()
            })
            .build()
            .unwrap();

        Self { shell }
    }

    /// Execute a command and return a structured result for the LLM.
    fn run(&mut self, command: &str) -> AgentResult {
        match self.shell.exec(command) {
            Ok(result) => AgentResult {
                success: result.exit_code == 0,
                stdout: truncate(&result.stdout, 4096),
                stderr: truncate(&result.stderr, 1024),
                exit_code: result.exit_code,
                error: None,
            },
            Err(RustBashError::LimitExceeded { limit_name, .. }) => AgentResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: -1,
                error: Some(format!("Resource limit exceeded: {limit_name}")),
            },
            Err(e) => AgentResult {
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                exit_code: -1,
                error: Some(format!("{e}")),
            },
        }
    }
}

struct AgentResult {
    success: bool,
    stdout: String,
    stderr: String,
    exit_code: i32,
    error: Option<String>,
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find a valid UTF-8 boundary at or before `max`
        let end = s.char_indices()
            .take_while(|(i, _)| *i < max)
            .last()
            .map(|(i, c)| i + c.len_utf8())
            .unwrap_or(0);
        format!("{}... [truncated, {} total bytes]", &s[..end], s.len())
    }
}
```

## Seeding with Task Context

Pre-populate the filesystem with task-specific data before handing control to the agent:

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

fn create_agent_for_task(task_files: HashMap<String, Vec<u8>>, task_description: &str) -> rust_bash::RustBash {
    let mut files = task_files;
    // Add the task description as a file the agent can reference
    files.insert(
        "/home/agent/TASK.md".into(),
        task_description.as_bytes().to_vec(),
    );

    RustBashBuilder::new()
        .files(files)
        .env(HashMap::from([
            ("HOME".into(), "/home/agent".into()),
            ("TASK_FILE".into(), "/home/agent/TASK.md".into()),
        ]))
        .cwd("/home/agent")
        .build()
        .unwrap()
}
```

## Tool Definition for Function Calling

Here's how you might describe the bash tool for an LLM API:

```json
{
  "name": "bash",
  "description": "Execute a bash command in a sandboxed environment. The environment has a virtual filesystem, 80+ Unix commands (grep, sed, awk, jq, find, curl, etc.), and full bash syntax (variables, loops, functions, pipes, redirections). State persists between calls.",
  "parameters": {
    "type": "object",
    "properties": {
      "command": {
        "type": "string",
        "description": "The bash command to execute"
      }
    },
    "required": ["command"]
  }
}
```

## Multi-Turn Conversation Pattern

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/data/users.csv".into(), b"name,email,role\nalice,a@x.com,admin\nbob,b@x.com,user\n".to_vec()),
    ]))
    .cwd("/data")
    .build()
    .unwrap();

// Turn 1: Agent explores the data
let r = shell.exec("ls /data && head -5 /data/users.csv").unwrap();
// LLM sees the file listing and CSV structure

// Turn 2: Agent processes the data
let r = shell.exec("awk -F, 'NR>1 && $3==\"admin\" { print $1, $2 }' /data/users.csv").unwrap();
// LLM sees: alice a@x.com

// Turn 3: Agent creates a report
shell.exec(r#"
    echo "# Admin Report" > /data/report.md
    echo "" >> /data/report.md
    awk -F, 'NR>1 && $3=="admin" { printf "- **%s** (%s)\n", $1, $2 }' /data/users.csv >> /data/report.md
"#).unwrap();

let r = shell.exec("cat /data/report.md").unwrap();
assert!(r.stdout.contains("# Admin Report"));
assert!(r.stdout.contains("**alice**"));
```

## Adding API Access for Agents

Enable controlled HTTP access so the agent can call external APIs:

```rust
use rust_bash::{RustBashBuilder, NetworkPolicy};
use std::time::Duration;

let mut shell = RustBashBuilder::new()
    .network_policy(NetworkPolicy {
        enabled: true,
        allowed_url_prefixes: vec![
            "https://api.myservice.com/".into(),
        ],
        timeout: Duration::from_secs(5),
        ..Default::default()
    })
    .build()
    .unwrap();

// Agent can call your API
// shell.exec("curl -s https://api.myservice.com/tasks | jq '.[0]'").unwrap();
```

## Custom Tools as Commands

Expose application-specific capabilities as custom commands:

```rust
use rust_bash::{RustBashBuilder, VirtualCommand, CommandContext, CommandResult};

/// A command that lets the agent signal task completion
struct DoneCommand;
impl VirtualCommand for DoneCommand {
    fn name(&self) -> &str { "task-done" }
    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let summary = args.join(" ");
        CommandResult {
            stdout: format!("TASK_COMPLETE: {summary}\n"),
            ..Default::default()
        }
    }
}

/// A command that provides structured context to the agent
struct ContextCommand;
impl VirtualCommand for ContextCommand {
    fn name(&self) -> &str { "get-context" }
    fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
        let user = ctx.env.get("USER").map(|s| s.as_str()).unwrap_or("unknown");
        CommandResult {
            stdout: format!("user={user}\ncwd={}\n", ctx.cwd),
            ..Default::default()
        }
    }
}

let mut shell = RustBashBuilder::new()
    .command(Box::new(DoneCommand))
    .command(Box::new(ContextCommand))
    .build()
    .unwrap();

let r = shell.exec("get-context").unwrap();
assert!(r.stdout.contains("cwd=/"));
```

## Protecting Against Malicious Scripts

The combination of execution limits, network policy, and InMemoryFs provides defense in depth:

1. **No host filesystem access** — InMemoryFs by default
2. **No network access** — disabled by default; requires explicit allow-list
3. **Resource bounds** — time, commands, output size all capped
4. **No process spawning** — all commands run in-process; no `std::process::Command`
5. **Structured errors** — `LimitExceeded` reports exactly which limit was hit

See [Execution Limits](execution-limits.md) for detailed configuration.
