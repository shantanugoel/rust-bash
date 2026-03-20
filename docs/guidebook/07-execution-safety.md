# Chapter 7: Execution Safety

## Overview

rust-bash is designed to run untrusted, AI-generated scripts. This chapter covers all safety mechanisms: execution limits, network policy, and the broader security model.

## Execution Limits

```rust
pub struct ExecutionLimits {
    pub max_call_depth: usize,           // default: 100
    pub max_command_count: usize,        // default: 10,000
    pub max_loop_iterations: usize,      // default: 10,000
    pub max_execution_time: Duration,    // default: 30s
    pub max_output_size: usize,          // default: 10MB
    pub max_string_length: usize,        // default: 10MB
    pub max_glob_results: usize,         // default: 100,000
    pub max_substitution_depth: usize,   // default: 50
    pub max_heredoc_size: usize,         // default: 10MB
    pub max_brace_expansion: usize,      // default: 10,000
}
```

### Enforcement Points

| Limit | Checked At |
|-------|-----------|
| `max_call_depth` | Every function call and `source` invocation |
| `max_command_count` | Every command dispatch (simple or compound) |
| `max_loop_iterations` | Each iteration of `for`, `while`, `until` loops |
| `max_execution_time` | Periodically during execution (wall-clock check) |
| `max_output_size` | Every stdout/stderr append |
| `max_string_length` | Variable assignment and string concatenation |
| `max_glob_results` | After glob expansion completes |
| `max_substitution_depth` | Nested `$()` command substitutions |
| `max_heredoc_size` | When processing here-document content |
| `max_brace_expansion` | When expanding `{1..N}` or `{a,b,...}` |

### Execution Counters

```rust
pub struct ExecutionCounters {
    pub command_count: usize,
    pub call_depth: usize,
    pub output_size: usize,
    pub start_time: Instant,
}
```

Counters are stored in `InterpreterState` and **reset at the start of each `exec()` call**. This means each `exec()` gets a fresh budget. Accumulated state (VFS, env) persists, but resource consumption is bounded per call.

### Limit Exceeded Behavior

When a limit is exceeded, execution stops immediately with a structured error:

```rust
RustBashError::LimitExceeded {
    limit_name: "max_loop_iterations",
    limit_value: 10_000,
    actual_value: 10_001,
}
```

The error propagates up and becomes the `ExecResult`'s stderr, with exit code 1. The sandbox remains usable for subsequent `exec()` calls — hitting a limit doesn't poison the sandbox.

## Network Policy

```rust
pub struct NetworkPolicy {
    pub enabled: bool,                     // default: false
    pub allowed_url_prefixes: Vec<String>, // e.g., ["https://api.example.com/"]
    pub allowed_methods: HashSet<String>,  // e.g., {"GET", "POST"}
    pub max_redirects: usize,             // default: 5
    pub max_response_size: usize,         // default: 10MB
    pub timeout: Duration,                // default: 30s
}
```

**Network is disabled by default.** The `curl` command checks the network policy before making any HTTP request. If networking is disabled or the URL doesn't match an allowed prefix, the command returns an error without making any network call.

### URL Validation

URL prefixes are matched literally. `"https://api.example.com/"` allows:
- `https://api.example.com/v1/data`
- `https://api.example.com/users?id=1`

But rejects:
- `https://api.example.com.evil.org/` (different domain)
- `http://api.example.com/` (different scheme)

### Redirect Safety

Even when a URL matches the allow list, redirects are followed only if:
1. The redirect count hasn't exceeded `max_redirects`
2. Each redirect target URL also matches an allowed prefix

This prevents an allowed URL from redirecting to a malicious endpoint.

## Security Model

### Threat Matrix

| Attack Vector | Mitigation | Status |
|---------------|------------|--------|
| Real filesystem access | All operations go through `VirtualFs` trait; `InMemoryFs` has zero `std::fs` calls | Core design |
| Process spawning | No `std::process::Command` anywhere; all commands are in-process Rust | Core design |
| Network exfiltration | `NetworkPolicy` disabled by default; URL prefix allow-listing when enabled | **(planned)** |
| Infinite loops | `max_loop_iterations` limit | **(planned)** |
| Fork bombs / recursion | `max_call_depth` limit | **(planned)** |
| Resource exhaustion | `max_command_count`, `max_execution_time`, `max_output_size` limits | **(planned)** |
| Memory exhaustion | `max_string_length`, `max_heredoc_size`, `max_brace_expansion` limits | **(planned)** |
| Path traversal | VFS path normalization handles `..`; OverlayFs restricts reads to specified base | Core design |
| Host time leakage | `SystemTime::now()` exposes real clock; future: inject clock abstraction | Known limitation |
| Lock poisoning | `parking_lot::RwLock` (non-poisoning) prevents command panics from killing VFS | Design decision |
| Glob DoS | `max_glob_results` prevents unbounded glob expansion | ✅ |
| Nested substitution | `max_substitution_depth` prevents `$($($(...)))` stack overflow | **(planned)** |

### What We Guarantee

1. **No real filesystem mutation** — when using `InMemoryFs` or `OverlayFs`, no file on the host is ever written or deleted.
2. **No process spawning** — the codebase contains zero calls to `std::process::Command`.
3. **No network access by default** — networking requires explicit opt-in via `NetworkPolicy`.
4. **Bounded execution** — configurable limits prevent any script from consuming unbounded resources.

### What We Don't Guarantee

1. **Timing side channels** — `SystemTime::now()` leaks real time. A determined attacker could measure execution timing.
2. **Memory usage** — we limit string sizes and output, but don't have a hard memory cap. A pathological script could still use significant memory within the per-limit bounds.
3. **CPU time** — `max_execution_time` is wall-clock, not CPU time. On a loaded system, a script might use more CPU time than expected.
4. **Deterministic output** — commands like `date` and `$RANDOM` produce non-deterministic output. For deterministic testing, inject fixed values via environment variables or clock abstraction.

## Configuration

```rust
let mut shell = RustBash::builder()
    .execution_limits(ExecutionLimits {
        max_command_count: 1_000,
        max_execution_time: Duration::from_secs(5),
        ..Default::default()
    })
    .network_policy(NetworkPolicy {
        enabled: true,
        allowed_url_prefixes: vec!["https://api.example.com/".into()],
        ..Default::default()
    })
    .build();
```

All limits have sensible defaults. You only need to configure limits you want to change.
