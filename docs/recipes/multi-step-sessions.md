# Multi-Step Sessions

## Goal

Maintain shell state — variables, files, working directory, functions — across multiple `exec()` calls. This is essential for interactive agents, REPL-like workflows, and multi-turn conversations.

## State That Persists

A `RustBash` instance preserves everything between `exec()` calls:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Step 1: Set up environment
shell.exec("HOME=/home/agent && USER=agent").unwrap();

// Step 2: Create files
shell.exec("mkdir -p /workspace && echo 'task data' > /workspace/input.txt").unwrap();

// Step 3: Process data — variables and files from steps 1 & 2 are available
shell.exec("cd /workspace").unwrap();
let result = shell.exec("echo \"User: $USER\" && cat input.txt").unwrap();
assert!(result.stdout.contains("User: agent"));
assert!(result.stdout.contains("task data"));

// Step 4: Check cwd persists
assert_eq!(shell.cwd(), "/workspace");
```

## What Persists vs. What Resets

| Persists across exec() | Resets each exec() |
|------------------------|--------------------|
| Environment variables | Execution counters (command count, timer) |
| Virtual filesystem (all files/dirs) | Control flow state |
| Current working directory | |
| Function definitions | |
| Shell options (errexit, pipefail, etc.) | |
| Trap handlers | |
| Positional parameters | |
| Exit code (accessible via `$?`) | |

## Building a Conversational Agent Loop

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits};
use std::collections::HashMap;
use std::time::Duration;

fn run_agent_session(tasks: &[&str]) {
    let mut shell = RustBashBuilder::new()
        .env(HashMap::from([
            ("HOME".into(), "/home/agent".into()),
        ]))
        .cwd("/home/agent")
        .execution_limits(ExecutionLimits {
            max_execution_time: Duration::from_secs(10),
            max_command_count: 5_000,
            ..Default::default()
        })
        .build()
        .unwrap();

    for (i, task) in tasks.iter().enumerate() {
        println!("--- Step {} ---", i + 1);
        match shell.exec(task) {
            Ok(result) => {
                if !result.stdout.is_empty() {
                    print!("{}", result.stdout);
                }
                if !result.stderr.is_empty() {
                    eprint!("{}", result.stderr);
                }
                if result.exit_code != 0 {
                    eprintln!("[exit: {}]", result.exit_code);
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                break;
            }
        }

        // Stop if the script called `exit`
        if shell.should_exit() {
            println!("Shell exited.");
            break;
        }
    }
}

// Simulate an agent that works in multiple steps
run_agent_session(&[
    "mkdir -p /work && cd /work",
    "echo 'Hello World' > greeting.txt",
    "cat greeting.txt | tr '[:lower:]' '[:upper:]' > result.txt",
    "cat result.txt",
]);
```

## Functions Persist Across Calls

Define helper functions in one call, use them in subsequent calls:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Define utility functions
shell.exec(r#"
    log() { echo "[$(date +%H:%M:%S)] $*"; }
    check_file() { test -f "$1" && echo "exists" || echo "missing"; }
"#).unwrap();

// Use them in later calls
let result = shell.exec("log 'Starting task'").unwrap();
assert!(result.stdout.contains("Starting task"));

shell.exec("echo data > /report.txt").unwrap();
let result = shell.exec("check_file /report.txt").unwrap();
assert_eq!(result.stdout, "exists\n");
```

## Checking Completion with is_input_complete

For REPL-like interfaces, use `RustBash::is_input_complete()` to detect whether the user's input is a complete statement or needs more lines:

```rust
use rust_bash::RustBash;

// Complete statements
assert!(RustBash::is_input_complete("echo hello"));
assert!(RustBash::is_input_complete("if true; then echo yes; fi"));

// Incomplete — need more input
assert!(!RustBash::is_input_complete("if true; then"));
assert!(!RustBash::is_input_complete("echo 'unterminated"));
assert!(!RustBash::is_input_complete("for i in 1 2 3; do"));
```

This is a static method — it doesn't need a shell instance.

## Trap Handlers Persist

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Set an EXIT trap
shell.exec("trap 'echo cleanup done' EXIT").unwrap();

// The trap fires at the end of each exec() call
let result = shell.exec("echo work").unwrap();
assert!(result.stdout.contains("work"));
assert!(result.stdout.contains("cleanup done"));
```

## Detecting the exit Command

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

shell.exec("echo working").unwrap();
assert!(!shell.should_exit());

shell.exec("exit 0").unwrap();
assert!(shell.should_exit());
// Don't send more exec() calls after this
```
