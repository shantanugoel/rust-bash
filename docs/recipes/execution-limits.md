# Execution Limits

## Goal

Configure resource bounds to prevent runaway scripts. Useful for untrusted input, AI agent sandboxes, and multi-tenant environments.

## Default Limits

Every `RustBash` instance starts with these defaults:

| Limit | Default | What it caps |
|-------|---------|--------------|
| `max_call_depth` | 25 | Recursive function call nesting |
| `max_command_count` | 10,000 | Total commands executed per `exec()` call |
| `max_loop_iterations` | 10,000 | Iterations per loop (`for`, `while`, `until`) |
| `max_execution_time` | 30 s | Wall-clock time per `exec()` call |
| `max_output_size` | 10 MB | Combined stdout + stderr size |
| `max_string_length` | 10 MB | Maximum length of any single variable value |
| `max_glob_results` | 100,000 | Glob expansion result count |
| `max_substitution_depth` | 50 | Nested `$(...)` command substitution depth |
| `max_heredoc_size` | 10 MB | Maximum heredoc content size |
| `max_brace_expansion` | 10,000 | Terms produced by brace expansion |

## Configuring Limits

Override defaults via `ExecutionLimits`:

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits};
use std::time::Duration;

let mut shell = RustBashBuilder::new()
    .execution_limits(ExecutionLimits {
        max_command_count: 500,
        max_loop_iterations: 100,
        max_execution_time: Duration::from_secs(5),
        max_output_size: 1024 * 1024, // 1 MB
        ..Default::default()  // keep defaults for all other limits
    })
    .build()
    .unwrap();
```

## Handling Limit Violations

When a limit is exceeded, `exec()` returns a `RustBashError::LimitExceeded` with details:

```rust
use rust_bash::{RustBashBuilder, RustBashError, ExecutionLimits};
use std::time::Duration;

let mut shell = RustBashBuilder::new()
    .execution_limits(ExecutionLimits {
        max_loop_iterations: 10,
        ..Default::default()
    })
    .build()
    .unwrap();

match shell.exec("for i in $(seq 1 100); do echo $i; done") {
    Ok(result) => println!("stdout: {}", result.stdout),
    Err(RustBashError::LimitExceeded { limit_name, limit_value, actual_value }) => {
        eprintln!("Limit hit: {limit_name} — allowed {limit_value}, got {actual_value}");
    }
    Err(e) => eprintln!("Error: {e}"),
}
```

## Preset Profiles

Here are suggested profiles for common scenarios:

### Strict — untrusted user input

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits};
use std::time::Duration;

let strict_limits = ExecutionLimits {
    max_call_depth: 10,
    max_command_count: 100,
    max_loop_iterations: 50,
    max_execution_time: Duration::from_secs(2),
    max_output_size: 64 * 1024,        // 64 KB
    max_string_length: 64 * 1024,       // 64 KB
    max_glob_results: 100,
    max_substitution_depth: 5,
    max_heredoc_size: 64 * 1024,        // 64 KB
    max_brace_expansion: 100,
};

let mut shell = RustBashBuilder::new()
    .execution_limits(strict_limits)
    .build()
    .unwrap();
```

### Moderate — AI agent sandbox

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits};
use std::time::Duration;

let agent_limits = ExecutionLimits {
    max_call_depth: 50,
    max_command_count: 5_000,
    max_loop_iterations: 1_000,
    max_execution_time: Duration::from_secs(10),
    max_output_size: 1024 * 1024,       // 1 MB
    max_string_length: 1024 * 1024,      // 1 MB
    max_glob_results: 10_000,
    max_substitution_depth: 20,
    max_heredoc_size: 1024 * 1024,       // 1 MB
    max_brace_expansion: 1_000,
};

let mut shell = RustBashBuilder::new()
    .execution_limits(agent_limits)
    .build()
    .unwrap();
```

### Permissive — trusted internal scripts

Use the defaults, which are already generous:

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits};

let mut shell = RustBashBuilder::new()
    .execution_limits(ExecutionLimits::default()) // explicit default
    .build()
    .unwrap();
```

## Counters Are Reset Per exec() Call

Limits apply independently to each `exec()` call. A shell that ran 9,999 commands in the previous call starts fresh:

```rust
use rust_bash::{RustBashBuilder, ExecutionLimits};

let mut shell = RustBashBuilder::new()
    .execution_limits(ExecutionLimits {
        max_command_count: 10,
        ..Default::default()
    })
    .build()
    .unwrap();

// Each exec() gets its own budget of 10 commands
shell.exec("echo 1; echo 2; echo 3").unwrap();  // 3 commands — OK
shell.exec("echo 4; echo 5; echo 6").unwrap();  // 3 more — OK (counter reset)
```

## What Counts as a "Command"

Each simple command execution increments the counter. This includes:
- External commands: `echo`, `grep`, `cat`, etc.
- Pipeline stages: `echo hello | grep hello` = 2 commands
- Commands inside loops, functions, and subshells
- Commands in `$(...)` substitutions

Builtins like `cd`, `export`, `set`, and variable assignments also count as commands.

---

## TypeScript: Configuring Execution Limits

The `rust-bash` npm package supports the same execution limits:

### Setting Limits

```typescript
import { Bash } from 'rust-bash';

const bash = await Bash.create(createBackend, {
  executionLimits: {
    maxCommandCount: 500,
    maxLoopIterations: 100,
    maxExecutionTimeSecs: 5,
    maxOutputSize: 1024 * 1024, // 1 MB
  },
});
```

All fields in `executionLimits` are optional — unset fields use defaults.

### Available Limits

| TypeScript Field | Default | What it caps |
|-----------------|---------|--------------|
| `maxCallDepth` | 100 | Recursive function call nesting |
| `maxCommandCount` | 10,000 | Total commands per `exec()` call |
| `maxLoopIterations` | 10,000 | Iterations per loop |
| `maxExecutionTimeSecs` | 30 | Wall-clock seconds per `exec()` call |
| `maxOutputSize` | 10,485,760 | Combined stdout + stderr bytes |
| `maxStringLength` | 10,485,760 | Maximum single variable value length |
| `maxGlobResults` | 100,000 | Glob expansion result count |
| `maxSubstitutionDepth` | 50 | Nested `$(...)` depth |
| `maxHeredocSize` | 10,485,760 | Maximum heredoc content size |
| `maxBraceExpansion` | 10,000 | Terms from brace expansion |

### Preset Profiles (TypeScript)

```typescript
// Strict — untrusted user input
const strictBash = await Bash.create(createBackend, {
  executionLimits: {
    maxCallDepth: 10,
    maxCommandCount: 100,
    maxLoopIterations: 50,
    maxExecutionTimeSecs: 2,
    maxOutputSize: 64 * 1024,
    maxStringLength: 64 * 1024,
    maxGlobResults: 100,
    maxSubstitutionDepth: 5,
    maxHeredocSize: 64 * 1024,
    maxBraceExpansion: 100,
  },
});

// Moderate — AI agent sandbox
const agentBash = await Bash.create(createBackend, {
  executionLimits: {
    maxCallDepth: 50,
    maxCommandCount: 5000,
    maxLoopIterations: 1000,
    maxExecutionTimeSecs: 10,
    maxOutputSize: 1024 * 1024,
  },
});

// Permissive — trusted scripts (defaults are already generous)
const trustedBash = await Bash.create(createBackend, {});
```
