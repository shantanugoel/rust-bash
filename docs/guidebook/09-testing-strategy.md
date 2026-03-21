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
    let mut sb = RustBashBuilder::new()
        .files(HashMap::from([
            ("/data.txt".into(), b"hello\nworld\nhello".to_vec()),
        ]))
        .build()
        .unwrap();
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
    let mut sb = RustBashBuilder::new()
        .files(HashMap::from([
            ("/a.txt".into(), vec![]),
            ("/b.txt".into(), vec![]),
            ("/dir/c.txt".into(), vec![]),
        ]))
        .build()
        .unwrap();
    let r = sb.exec("ls -la /").unwrap();
    insta::assert_snapshot!(r.stdout);
}
```

Snapshots are the most efficient way to catch regressions across 70+ commands. When behavior intentionally changes, review and update snapshots with `cargo insta review`.

### Differential Testing — Comparison Tests

Comparison tests verify that rust-bash produces the same stdout, stderr, and exit code as real `/bin/bash` for a corpus of shell scripts. Each test case is a TOML entry with a bash script and recorded expected output. Tests run against rust-bash only during `cargo test` — no real bash needed. A separate recording mode re-captures expected output from real bash.

**File location**: `tests/fixtures/comparison/` — organized by feature area (quoting, expansion, control flow, etc.).

**Runner**: `tests/comparison.rs` uses `datatest-stable` to discover all `.toml` fixture files and generate one `#[test]` per file. Within each file, all cases run sequentially; failures are collected and reported together.

**What's covered** (157 test cases across 19 fixture files):
- Quoting (single, double, backslash escaping)
- Parameter expansion (defaults, alternatives, substitution, length, case modification)
- Command substitution, arithmetic expansion, brace expansion, tilde expansion
- Word splitting (IFS variations)
- Globbing (`*`, `?`, `[...]`)
- Redirections (`>`, `>>`, `2>`, `<`, here-documents, here-strings)
- Pipelines (simple and multi-stage)
- Control flow (`if`, `for`, `while`, `case`, logical operators)
- Functions (definition, local variables, return values)

### Differential Testing — Spec Tests

Spec tests verify command implementations (`grep`, `sed`, `awk`, `jq`) against manually written expected output. Unlike comparison tests, spec tests do **not** have a recording mode — expected output is written by hand because our implementations are intentionally subset.

**File location**: `tests/fixtures/spec/` — organized by command (`grep/`, `sed/`, `awk/`, `jq/`).

**Runner**: `tests/spec_tests.rs` — structurally identical to the comparison runner but reads from `tests/fixtures/spec/` and does not support recording.

**What's covered** (197 test cases across 14 fixture files):
- **grep**: literal matching, regex, flags (`-i`, `-v`, `-c`, `-n`, `-l`, `-r`, `-E`, `-F`, `-w`, `-o`, `-q`, `-A`/`-B`/`-C`, `-e`, `-x`, `-m`, `-h`)
- **sed**: substitution, address ranges, delete/print/append/insert/change, transliterate (`y///`), hold space, in-place edit (`-i`), branching
- **awk**: field splitting, patterns, built-in functions, arithmetic, associative arrays
- **jq**: basic filters, pipe operator, types, comparison, built-in functions (`map`, `select`, `keys`, `sort`, `reduce`, `length`, `split`, `join`, `test`, etc.), string interpolation, alternative operator, output flags (`-r`, `-c`, `-s`, `-S`, `-n`, `-j`), `--arg`/`--argjson`

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
│   │   ├── memory.rs          # InMemoryFs implementation
│   │   ├── readwrite_tests.rs # #[cfg(test)] ReadWriteFs tests
│   │   ├── overlay_tests.rs   # #[cfg(test)] OverlayFs tests
│   │   ├── mountable_tests.rs # #[cfg(test)] MountableFs tests
│   │   └── tests.rs           # #[cfg(test)] shared VFS trait tests
│   ├── commands/
│   │   └── mod.rs             # #[cfg(test)] mod tests — command unit tests (inline)
│   ├── interpreter/
│   │   ├── mod.rs             # #[cfg(test)] mod tests — parse + word expansion unit tests
│   │   └── expansion.rs       # word expansion engine (no inline tests)
│   └── parser_smoke_tests.rs  # Smoke tests for brush-parser API surface
└── tests/
    ├── integration.rs         # End-to-end tests through RustBash::exec()
    ├── comparison.rs          # Comparison test runner (rust-bash vs recorded bash output)
    ├── spec_tests.rs          # Spec test runner (awk, grep, sed, jq)
    ├── common/
    │   └── mod.rs             # Shared data model and test execution logic
    ├── filesystem_backends.rs # VFS backend integration tests
    ├── cli.rs                 # CLI entry-point tests
    ├── ffi.rs                 # FFI/C-binding tests
    ├── fixtures/
    │   ├── comparison/        # TOML fixtures recorded from real bash
    │   │   ├── basic_echo.toml
    │   │   ├── quoting/
    │   │   ├── expansion/
    │   │   ├── word_splitting/
    │   │   ├── globbing/
    │   │   ├── redirections/
    │   │   ├── pipes/
    │   │   ├── control_flow/
    │   │   ├── functions/
    │   │   └── here_documents/
    │   └── spec/              # Manually written spec tests
    │       ├── basic_commands.toml
    │       ├── grep/
    │       ├── sed/
    │       ├── awk/
    │       └── jq/
    └── snapshots/             # insta snapshot files
```

## CI Pipeline

1. `cargo fmt --check` — formatting
2. `cargo clippy -- -D warnings` — linting
3. `cargo test` — all unit + integration tests (including insta snapshot tests)
4. `cargo insta review` — review any new or changed snapshots **(run locally before committing)**

> **Fuzzing** is not yet set up — no `fuzz/` directory exists. Adding `cargo fuzz` targets is aspirational future work.

## TOML Fixture Format

Both comparison and spec tests use the same TOML format. Each fixture file contains a `[[cases]]` array:

```toml
# tests/fixtures/comparison/expansion/parameter_default.toml

[[cases]]
name = "unset_default_with_colon"
script = 'echo "${UNSET:-fallback}"'
stdout = "fallback\n"
stderr = ""
exit_code = 0

[[cases]]
name = "skip_example"
script = "echo $'hello\\tworld'"
skip = "rust-bash does not implement ANSI-C quoting"
stdout = ""
exit_code = 0
```

### Available fields

| Field | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `name` | string | yes | — | Unique test case name (used in failure output) |
| `script` | string | yes | — | Bash script to execute |
| `stdout` | string | no | `""` | Expected stdout (exact match) |
| `stderr` | string | no | `""` | Expected stderr (exact match) |
| `exit_code` | integer | no | `0` | Expected exit code |
| `stderr_contains` | string | no | — | Partial stderr match (mutually exclusive with `stderr_ignore`) |
| `stderr_ignore` | boolean | no | `false` | Skip stderr comparison entirely |
| `stdin` | string | no | — | Content piped to the script's stdin |
| `expect_error` | boolean | no | `false` | If true, the test passes when `exec()` returns `Err` |
| `files` | table | no | `{}` | VFS files to seed before running (key = path, value = content) |
| `env` | table | no | `{}` | Extra environment variables (merged with base env) |
| `skip` | string | no | — | If set, skip this case and print the reason |

### Base environment

All test cases run with a controlled environment (no inherited host variables):

- `HOME=/root`
- `USER=testuser`
- `TZ=UTC`
- `LC_ALL=C`
- `PATH=/usr/local/bin:/usr/bin:/bin`

The `env` field in a test case adds to (or overrides) these defaults.

### Execution limits

All cases run with execution limits to prevent hangs: max 10,000 loop iterations and 5-second wall-clock timeout.

## Adding New Test Cases

**Comparison tests** — to test a shell language feature against real bash:

1. Find the appropriate TOML file in `tests/fixtures/comparison/` (or create a new one in the right subdirectory).
2. Add a `[[cases]]` entry with `name`, `script`, and the expected `stdout`/`stderr`/`exit_code`.
3. Run `cargo test --test comparison` to verify the case passes.

If you don't know the expected output, use recording mode to capture it from real bash:

```bash
RECORD_FIXTURES=1 cargo test --test comparison
```

This runs each script against `/bin/bash` and overwrites the `stdout`, `stderr`, and `exit_code` fields in-place (preserving comments and formatting via `toml_edit`). Review the diffs, then run `cargo test` to confirm rust-bash matches.

> **Note**: Recording mode stages `files` entries into a real temp directory. Scripts using absolute VFS paths (e.g., `/tmp/test.txt`) may see different paths than in the VFS sandbox. For such cases, prefer relative paths in scripts, or write expected output manually and mark the test with `skip` for recording.

**Spec tests** — to test a command implementation (grep, sed, awk, jq):

1. Find the appropriate TOML file in `tests/fixtures/spec/` (or create a new one).
2. Add a `[[cases]]` entry with manually written expected output.
3. Run `cargo test --test spec_tests` to verify.

Spec tests have no recording mode — expected output is always hand-written.

## Marking Known Failures

If rust-bash doesn't yet match bash for a particular case, mark it with `skip` and a reason:

```toml
[[cases]]
name = "ansi_c_quoting"
script = "echo $'hello\\tworld'"
skip = "rust-bash does not implement ANSI-C quoting ($'...')"
stdout = "hello\tworld\n"
exit_code = 0
```

Skipped cases are printed during test runs (e.g., `SKIP ansi_c_quoting: rust-bash does not implement ANSI-C quoting`) but do not cause failures. Remove the `skip` field once the feature is implemented.

## Re-Recording Fixtures

Fixtures should be periodically re-recorded to catch regressions against newer bash versions and to update expected output as rust-bash behavior improves:

```bash
RECORD_FIXTURES=1 cargo test --test comparison
```

**Workflow**:
1. Run the recording command locally (requires `/bin/bash` on the host).
2. Review the git diff — verify that changes are expected (e.g., a fixed bug now produces correct output).
3. Run `cargo test` without `RECORD_FIXTURES` to confirm rust-bash passes with the updated fixtures.
4. Commit the updated fixture files.

Recording mode skips cases marked with `skip`. Each script runs with a 10-second timeout and the same controlled environment as normal test execution, ensuring reproducible results.

> **Note**: Recording mode uses `std::process::Command` to invoke real `/bin/bash`. This is the **only** code path in the project that shells out to an external process, and it lives in test code only — never in library code.

## Testing Conventions

- **Test names describe behavior, not implementation**: `fn pipe_chains_stdout_to_stdin()` not `fn test_pipeline()`
- **One assertion per concept**: test one behavior aspect per test function
- **Use builder helpers**: create test-specific sandbox builders to reduce boilerplate
- **Test error cases too**: verify that invalid inputs produce correct error messages and exit codes
- **Don't test brush-parser**: we trust the parser. Test our interpretation of its output.
