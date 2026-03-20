# Error Handling

## Goal

Handle the different error types returned by rust-bash, distinguish between script failures and execution errors, and use shell options like `set -e` for fail-fast behavior.

## Error Types

`exec()` returns `Result<ExecResult, RustBashError>`. It's important to understand what's an error vs. a normal result:

| Situation | Return type | Example |
|-----------|------------|---------|
| Command exits non-zero | `Ok(ExecResult { exit_code: 1, .. })` | `grep pattern /no-match` |
| Command not found | `Ok(ExecResult { exit_code: 127, .. })` | `nonexistent_cmd` |
| Parse error | `Err(RustBashError::Parse(_))` | `echo 'unterminated` |
| Readonly variable | `Err(RustBashError::Execution(_))` | `readonly X=1; X=2` |
| Limit exceeded | `Err(RustBashError::LimitExceeded { .. })` | Infinite loop with low limit |
| FS error (builder) | `Err(RustBashError::Vfs(_))` | Invalid path in builder |
| Timeout | `Err(RustBashError::Timeout)` | Script exceeds time limit |

## Matching on Error Variants

```rust
use rust_bash::{RustBashBuilder, RustBashError};

let mut shell = RustBashBuilder::new().build().unwrap();

let input = "some user input here";
match shell.exec(input) {
    Ok(result) => {
        if result.exit_code == 0 {
            println!("Success: {}", result.stdout);
        } else {
            eprintln!("Command failed (exit {}): {}", result.exit_code, result.stderr);
        }
    }
    Err(RustBashError::Parse(msg)) => {
        eprintln!("Syntax error: {msg}");
    }
    Err(RustBashError::LimitExceeded { limit_name, limit_value, actual_value }) => {
        eprintln!("Limit '{limit_name}' exceeded: {actual_value} > {limit_value}");
    }
    Err(RustBashError::Execution(msg)) => {
        eprintln!("Runtime error: {msg}");
    }
    Err(RustBashError::Timeout) => {
        eprintln!("Script timed out");
    }
    Err(e) => {
        eprintln!("Other error: {e}");
    }
}
```

## Script-Level Error Handling

### set -e (errexit)

Makes the script stop on the first command that fails:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Without set -e: all commands run regardless of failures
let result = shell.exec("false; echo still-runs").unwrap();
assert_eq!(result.stdout, "still-runs\n");

// With set -e: stops at first failure (returns Ok with non-zero exit code)
let result = shell.exec("set -e; false; echo never-runs").unwrap();
assert_ne!(result.exit_code, 0);
assert!(!result.stdout.contains("never-runs"));
```

### set -e exceptions

`set -e` does NOT trigger on failures in these contexts:
- `if` conditions: `if false; then ...` — the `false` is expected
- `&&` / `||` left-hand side: `false || echo recovered`
- `!` negated pipelines: `! false` succeeds
- `while`/`until` conditions

### set -u (nounset)

Error on unset variable expansion:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Without set -u: unset variables expand to empty string
let result = shell.exec("echo \"$UNDEFINED\"").unwrap();
assert_eq!(result.stdout, "\n");

// With set -u: unset variables cause an error
let result = shell.exec("set -u; echo $UNDEFINED");
assert!(result.is_err());
```

### set -o pipefail

Report failures from any stage in a pipeline, not just the last:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Without pipefail: exit code is from the last command in the pipeline
let result = shell.exec("false | echo hello").unwrap();
assert_eq!(result.exit_code, 0); // echo succeeded, so exit is 0

// With pipefail: exit code is the rightmost non-zero
let result = shell.exec("set -o pipefail; false | echo hello").unwrap();
assert_ne!(result.exit_code, 0); // false's exit code propagates
```

## Using trap for Cleanup

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

let result = shell.exec(r#"
    trap 'echo "cleaning up..."' EXIT
    echo "doing work"
    false
    echo "after failure"
"#).unwrap();

// EXIT trap fires at end of exec() regardless of how the script ended
assert!(result.stdout.contains("cleaning up..."));
```

## Recovering After Errors

A `RustBash` instance remains usable after errors. Parse errors leave state completely untouched. Limit and runtime errors may leave the shell in a partially modified state (variables set, files created, etc.), but the shell remains functional:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Set up some state
shell.exec("FOO=hello").unwrap();

// A parse error doesn't corrupt state
let result = shell.exec("echo 'unterminated");
assert!(result.is_err());

// Previous state is intact
let result = shell.exec("echo $FOO").unwrap();
assert_eq!(result.stdout, "hello\n");
```

## The ${VAR:?message} Pattern

Use parameter expansion to fail with a clear message on missing variables:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();

// Fails with a descriptive error
let result = shell.exec("echo ${DB_HOST:?DB_HOST must be set}");
assert!(result.is_err());

// Works when set
shell.exec("DB_HOST=localhost").unwrap();
let result = shell.exec("echo ${DB_HOST:?DB_HOST must be set}").unwrap();
assert_eq!(result.stdout, "localhost\n");
```
