# Chapter 10: Implementation Plan

## Milestones Overview

| # | Milestone | Goal |
|---|-----------|------|
| M1 | Core Shell | Production interpreter + VFS trait + ~35 commands |
| M2 | Text Processing | awk, sed, jq, diff + remaining text commands |
| M3 | Execution Safety | Limits enforcement, network policy |
| M4 | Filesystem Backends | OverlayFs, ReadWriteFs, MountableFs |
| M5 | Integration | C FFI, WASM, CLI binary, AI SDK wrapper |

---

## Milestone 1: Core Shell

**Goal**: A correct, reliable interpreter that handles the bash features AI agents actually use.

### M1.1 — VFS Trait Extraction ✅

Extract `VirtualFs` trait from `InMemoryFs`. Update `CommandContext` to `&dyn VirtualFs`, `InterpreterState` to `Arc<dyn VirtualFs>`. Use `parking_lot::RwLock` to avoid lock poisoning.

**Why first**: every subsequent component depends on the trait abstraction.

### M1.2 — Compound List Output Accumulation ✅

Fix compound list execution to accumulate stdout/stderr across all items. `echo a; echo b` correctly returns `"a\nb\n"`.

### M1.3 — Word Splitting and Quoting Correctness ✅

Implement IFS-based word splitting after variable expansion. Respect quoting rules: double-quoted expansions don't word-split, single-quoted are literal. Handle `"$@"` vs `"$*"`.

### M1.4 — Command Substitution ✅

Implement `$(...)` and backtick expansion. Requires interior mutability refactor (`RefCell` or `&mut` restructuring) since command substitution executes commands during word expansion.

### M1.5 — Exec Callback for Sub-Commands ✅

Add `exec` callback to `CommandContext` so commands can invoke sub-commands. Implement `eval` and `source` builtins. This unblocks `xargs`, `find -exec`, etc.

### M1.6 — test/[ and [[ (Extended Test) ✅

Implement conditional expressions: file tests (`-f`, `-d`, `-e`), string tests (`-z`, `-n`, `=`), numeric comparisons (`-eq`, `-lt`, etc.). Implement `[[ ]]` with pattern matching and regex.

### M1.7 — break/continue ✅

Implement loop control flow with optional numeric arguments (`break 2`). Uses a signal mechanism that propagates through nested execution.

### M1.8 — Glob Expansion ✅

Implement `VirtualFs::glob()` on InMemoryFs. Integrate into word expansion for unquoted wildcards. Include a simple numeric guard against unbounded results (formalized as part of `ExecutionLimits` in M3.1).

### M1.9 — Brace Expansion ✅

Implement `{a,b,c}` alternation and `{1..10}` sequence expansion. Include a simple numeric guard against unbounded expansion (formalized as part of `ExecutionLimits` in M3.1).

### M1.10 — Here-Documents and Here-Strings ✅

Handle `<<EOF` and `<<<word`. The heredoc body is already in the AST from brush-parser — just feed it as stdin. Support variable expansion within unquoted heredocs.

### M1.11 — Arithmetic Expansion ✅

Implement `$((...))` evaluator: arithmetic operators, comparisons, boolean logic, ternary, variable references, increment/decrement. Implement `let` and `((...))`.

### M1.12 — Functions and Local Variables ✅

Store function definitions. Implement function call with positional parameters. `local` for function-scoped variables. `return` builtin. Distinguish exported vs non-exported variables.

### M1.13 — Case Statements ✅

Implement `case` with glob pattern matching, `|` alternation, `;;`/`;&`/`;;&` terminators.

### M1.14 — Additional Core Commands ✅

File ops: `cp`, `mv`, `rm`, `tee`, `stat`, `chmod`. Text: `cut`, `printf`, `rev`, `fold`, `nl`. Navigation: `find`, `realpath`. Utilities: `expr`, `date`, `sleep`, `env`, `which`, `xargs`, `read`, `base64`, `md5sum`, `sha256sum`, `whoami`, `hostname`, `uname`. Minimal `trap EXIT` support.

### M1.15 — Error Handling ✅

Define `RustBashError` enum. All public APIs return `Result<T, RustBashError>`. Implement `set -e`, `set -u`, `set -o pipefail`.

---

## Milestone 2: Text Processing

### M2.1 — grep (Full) ✅

Add `regex` crate. Support `-E`, `-G`, `-P`, `-F`, `-n`, `-l`, `-L`, `-r`, `-R`, `-o`, `-A`/`-B`/`-C`, `-v`, `-c`, `-w`, `-x`, `-H`, `-h`, `-q`, `-m`, `-e`, `-f`, `--include`, `--exclude`.

### M2.2 — sed ✅

Core commands: `s///`, `d`, `p`, `q`, `a`, `i`, `c`. Address types: line number, `$`, `/regex/`, ranges. Hold space for multi-line operations. `-i` for in-place VFS edit.

### M2.3 — awk ✅

Field splitting, patterns, actions, BEGIN/END, built-in variables (NR, NF, FS), control flow, built-in functions, associative arrays. Start with 80/20 subset.

### M2.4 — jq ✅

Via `jaq-core` crate. Common filters: `.field`, `.[]`, `select()`, `map()`, `keys`, `length`, `|`. Flags: `-r`, `-e`, `-c`, `-S`.

### M2.5 — diff ✅

Via `similar` crate. Unified (`-u`), context (`-c`), and normal diff formats. `-r` for recursive directory diff.

### M2.6 — Remaining Text Commands ✅

`comm`, `join`, `fmt`, `column`, `expand`/`unexpand`, `yes`, `tac`.

---

## Milestone 3: Execution Safety

### M3.1 — Execution Limits Enforcement ✅

Add `ExecutionLimits` + `ExecutionCounters` to state. Check limits at command dispatch, function calls, loop iterations, output appends, and wall-clock time. Return structured `LimitExceeded` errors.

### M3.2 — Network Access Control ✅

Implement `NetworkPolicy`. Sandboxed `curl` validates URL against allow-list before HTTP request. Method restrictions, redirect following, response size limits.

---

## Milestone 4: Filesystem Backends

### M4.1 — OverlayFs ✅

Read from real directory, write to in-memory layer. Whiteout tracking for deletions. Merged directory listings.

### M4.2 — ReadWriteFs ✅

Thin `std::fs` wrapper. Optional path restriction (chroot-like).

### M4.3 — MountableFs ✅

Composite backend with path-based delegation. Longest-prefix mount matching.

---

## Milestone 5: Integration

### M5.1 — CLI Binary

Static binary. `--files`, `--cwd`, `--env` flags. Interactive REPL. `--json` output mode.

### M5.2 — C FFI

Stable C ABI: `rust_bash_create`, `rust_bash_exec`, `rust_bash_free`. JSON config. Generated C header.

### M5.3 — WASM Target

`wasm32-unknown-unknown` + `wasm-bindgen`. JavaScript wrapper. npm package with TypeScript types.

### M5.4 — AI SDK Integration

Tool definitions for OpenAI/Anthropic function calling. TypeScript wrapper for Vercel AI SDK.

---

## Build Order and Dependencies

```
M1.1 (VFS trait) ──┬── M1.2 (output fix) ── M1.3 (word splitting)
                   │          │
                   │   M1.4 (cmd substitution) ── M1.5 (exec callback)
                   │
                   ├── M1.6 (test/[[)
                   ├── M1.7 (break/continue)
                   ├── M1.8 (globs) ── M1.9 (brace expansion)
                   ├── M1.10 (heredocs)
                   ├── M1.11 (arithmetic)
                   ├── M1.12 (functions) ← depends on M1.6
                   ├── M1.13 (case)
                   ├── M1.14 (commands)
                   └── M1.15 (errors)
                         │
M2.1–M2.6 ──────────────┘  (depend on M1 interpreter)
M3.1 (limits) ──────────── (integrates into interpreter from M1)
M3.2 (network) ──────────  (curl needs network policy)
M4.1–M4.3 ──────────────── (depend on M1.1 VFS trait)
M5.1–M5.4 ──────────────── (depend on M1 + M2 for usefulness)
```

**Recommended order**: M1.1 → M1.2 → M1.3 → M1.4 → M1.5 → M1.6 → M1.7 → M1.8/M1.9/M1.10/M1.11 (parallel) → M1.12 → M1.13 → M1.14 → M1.15 → M3.1 → M2.1 → M2.2 → M2.3 → M2.4 → M4.1 → M5.1 → M5.2 → M5.3

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| brush-parser breaking changes | Medium | Medium | Pin to git rev, consider adapter layer |
| awk/sed complexity explosion | High | Medium | Implement 80/20 subset first |
| Word expansion edge cases | Medium | High | Differential testing against real bash |
| WASM binary size too large | Medium | Medium | Feature-gate heavy commands |
| Command substitution refactoring | Medium | Medium | RefCell approach; budget time for cascade |
| Lock poisoning from panics | Medium | High | parking_lot::RwLock (non-poisoning) |

---

## Open Questions

1. **Adapter layer for brush-parser types?** Wrapping AST types insulates from upstream changes but adds code.

2. **awk/sed 80/20 scope**: Proposed minimum:
   - **awk**: `{print $N}`, `-F`, `NR`/`NF`, `BEGIN`/`END`, conditions, `printf`, `split`
   - **sed**: `s///g`, `d`, `p`, `q`, addresses, ranges. Ship without hold space first.

3. **Async vs sync API**: Should `exec()` be async? Recommend: start sync, add async wrapper later if needed for timeout or concurrent pipes.

4. **Error message compatibility**: Match bash format (`bash: <cmd>: <msg>`) but not exact wording.

5. **Variable scoping**: Target: dynamic scoping matching bash — all variables are visible to called functions. `local` creates function-scoped variables. `export` marks variables for child process inheritance (not directly relevant in our no-process-spawning model, but maintained for script compatibility).
