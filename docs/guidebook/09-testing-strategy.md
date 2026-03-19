# Chapter 9: Testing Strategy

## Overview

rust-bash needs high test confidence because it's a security boundary — incorrect behavior could leak host resources or produce wrong results for agents. This chapter covers all testing approaches.

## Test Categories

### Unit Tests

Each component tested in isolation:

- **VFS operations**: file CRUD, directory operations, path normalization, symlinks, glob matching
- **Word expansion**: variable expansion, quoting, command substitution, arithmetic, tilde, brace expansion
- **Commands**: each command tested with known inputs and expected outputs
- **Execution limits**: verify each limit type is enforced correctly

Unit tests live alongside the code in `#[cfg(test)]` modules.

### Integration Tests

End-to-end tests through the `RustBash::exec()` API:

```rust
#[test]
fn pipeline_with_redirect() {
    let mut sb = RustBash::builder()
        .files([("/data.txt", "hello\nworld\nhello")])
        .build();
    let r = sb.exec("grep hello /data.txt | wc -l > /count.txt && cat /count.txt").unwrap();
    assert_eq!(r.stdout, "2\n");
    assert_eq!(r.exit_code, 0);
}
```

Integration tests verify:
- Multi-command scripts with pipelines, redirections, and control flow
- State persistence across `exec()` calls (files, env, cwd)
- Error handling (parse errors, command errors, limit exceeded)
- Builder configuration (files, env, cwd, limits)

Integration tests live in `tests/`.

### Snapshot Tests

Use the `insta` crate for snapshot testing. Run a command through the sandbox and compare the output against a saved snapshot:

```rust
#[test]
fn snapshot_ls_output() {
    let mut sb = RustBash::builder()
        .files([("/a.txt", ""), ("/b.txt", ""), ("/dir/c.txt", "")])
        .build();
    let r = sb.exec("ls -la /").unwrap();
    insta::assert_snapshot!(r.stdout);
}
```

Snapshots are the most efficient way to catch regressions across 70+ commands. When behavior intentionally changes, review and update snapshots with `cargo insta review`.

### Differential Testing Against just-bash

Run the same command corpus through both just-bash and rust-bash, comparing:
- stdout
- stderr (format may differ, but semantics should match)
- exit codes

This is the highest-value correctness test. just-bash is the behavioral reference implementation.

**Test corpus**: maintain a file of commands with expected behavior:
```
# command | expected_stdout | expected_exit_code
echo hello | hello\n | 0
echo $((2+3)) | 5\n | 0
cat nonexistent 2>&1 | cat: nonexistent: No such file or directory\n | 1
```

### Bash Compatibility Tests

Port a subset of bash's own test suite, focused on features AI agents use:
- Variable expansion (simple, default values, string operations)
- Control flow (if/for/while/case)
- Pipelines and redirections
- Command substitution
- Arithmetic expansion
- Glob expansion

We do not aim for 100% bash compatibility — only the subset that matters for agent usage.

### Fuzzing

Use `cargo fuzz` to feed arbitrary strings through the full pipeline:

```
arbitrary string → tokenize → parse → interpret → VFS
```

The fuzzer should verify:
- No panics (catch_unwind everything)
- No infinite loops (execution limits must catch them)
- No real FS access (monitor with strace in CI)
- No unbounded memory growth

Start fuzzing early — don't defer to later milestones. The parser → interpreter boundary is a rich attack surface.

## Test Organization

```
rust-bash/
├── src/
│   ├── vfs/
│   │   └── memory.rs        # #[cfg(test)] mod tests — VFS unit tests
│   ├── commands/
│   │   └── text.rs           # #[cfg(test)] mod tests — command unit tests
│   └── interpreter/
│       └── expand.rs         # #[cfg(test)] mod tests — expansion unit tests
├── tests/
│   ├── interpreter.rs        # Integration: compound commands, control flow
│   ├── vfs.rs                # Integration: VFS backend behavior
│   ├── commands.rs           # Integration: command behavior through sandbox
│   ├── bash_compat.rs        # Bash compatibility corpus
│   └── snapshots/            # insta snapshot files
└── fuzz/
    └── fuzz_targets/
        └── exec.rs           # Fuzz target: arbitrary string → shell.exec()
```

## CI Pipeline

1. `cargo fmt --check` — formatting
2. `cargo clippy -- -D warnings` — linting
3. `cargo test` — all unit + integration tests
4. `cargo insta test` — snapshot tests
5. `cargo fuzz run exec -- -max_total_time=60` — fuzzing (limited time in CI)
6. `cargo build --target wasm32-unknown-unknown` — verify WASM compilation

## Testing Conventions

- **Test names describe behavior, not implementation**: `fn pipe_chains_stdout_to_stdin()` not `fn test_pipeline()`
- **One assertion per concept**: test one behavior aspect per test function
- **Use builder helpers**: create test-specific sandbox builders to reduce boilerplate
- **Test error cases too**: verify that invalid inputs produce correct error messages and exit codes
- **Don't test brush-parser**: we trust the parser. Test our interpretation of its output.
