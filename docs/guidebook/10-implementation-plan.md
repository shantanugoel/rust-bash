# Chapter 10: Implementation Plan

## Milestones Overview

| # | Milestone | Goal |
|---|-----------|------|
| M1 | Core Shell | Production interpreter + VFS trait + ~35 commands |
| M2 | Text Processing | awk, sed, jq, diff + remaining text commands |
| M3 | Execution Safety | Limits enforcement, network policy |
| M4 | Filesystem Backends | OverlayFs, ReadWriteFs, MountableFs |
| M5 | Integration | C FFI, WASM, CLI binary, AI SDK wrapper |
| M6 | Shell Language Completeness | Arrays, shopt, process substitution, special vars, advanced redirections, missing builtins |
| M7 | Command Coverage & Discoverability | Missing commands, `--help` for all commands |
| M8 | Embedded Runtimes & Data Formats | Python, JavaScript, SQLite, yq, xan |
| M9 | Platform & API Features | Cancellation, lazy files, AST transforms, sandbox API |

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

### M5.1 — CLI Binary ✅

Static binary. `--files`, `--cwd`, `--env` flags. Interactive REPL. `--json` output mode.

### M5.2 — C FFI ✅

Stable C ABI: 6 exported functions (`rust_bash_create`, `rust_bash_exec`, `rust_bash_result_free`, `rust_bash_free`, `rust_bash_last_error`, `rust_bash_version`). JSON config. Generated C header.

### M5.3 — WASM Target

`wasm32-unknown-unknown` + `wasm-bindgen`. JavaScript wrapper. npm package with TypeScript types.

**Design exploration (do before implementing):** Evaluate `napi-rs v3` (supports compiling the same Rust crate to both native Node.js addons and WASM from a single codebase) vs separate `wasm-bindgen` + `napi-rs` builds. Compare bundle size, API ergonomics, and maintenance cost. The dual-target capability of napi-rs v3 may allow M5.3 and M5.4 to share a single binding layer — investigate whether this simplifies or constrains the API surface.

### M5.4 — AI SDK Integration

Tool definitions for OpenAI/Anthropic function calling. TypeScript wrapper for Vercel AI SDK.

**Design exploration (do before implementing):** The TypeScript/JS package should offer a **native Node.js addon** (via napi-rs) as the primary backend for server-side AI agents, with WASM as an automatic fallback for browsers and edge runtimes. Investigate a unified `@rust-bash/core` package that auto-detects the environment. Compare this approach against shipping separate `@rust-bash/node` and `@rust-bash/wasm` packages.

**Custom commands via TypeScript (and other language interfaces):** The `VirtualCommand` trait (`fn execute(&self, args, ctx) -> CommandResult`) maps cleanly to a JS/TS callback. Explore three approaches: (1) a `JsBridgeCommand` Rust struct that implements `VirtualCommand` but delegates to a registered TS callback via napi-rs `ThreadsafeFunction` (native) or `wasm-bindgen` imported function (WASM); (2) a catch-all `commandResolver` fallback for dynamic command sets (like bash's `command_not_found_handle`); (3) optionally exposing VFS read/write methods to TS callbacks, or letting them shell out via the existing `exec` callback on `CommandContext`. The same pattern generalizes to the C FFI (M5.2) by accepting function pointers for custom command dispatch. Approach (1) + (2) covers most use cases; (3) can be deferred.

---

## Milestone 6: Shell Language Completeness

**Goal**: Close remaining bash language gaps so AI-generated scripts that use arrays, shopt, advanced builtins, and process substitution work without modification.

### M6.1 — Indexed and Associative Arrays

Extend `Variable` to hold array data (`Option<Box<ArrayData>>` with `Indexed(BTreeMap<usize, String>)` and `Associative(HashMap<String, String>)` variants). Handle `AssignmentValue::Array` and `ArrayElementName` from brush-parser (currently dropped). Implement `${arr[@]}`, `${arr[*]}`, `${#arr[@]}`, `${!arr[@]}`, `${arr[@]:offset:length}`, array `+=()` append, and `unset arr[n]` (sparse — no reindexing).

**Why first in M6**: Arrays are the critical path — `$PIPESTATUS`, `BASH_REMATCH`, `mapfile`, `read -a`, and `declare -A` all depend on this.

### M6.2 — `$PIPESTATUS` and `BASH_REMATCH` as Arrays

Expose the `exit_codes` vector already collected in `execute_pipeline` as the `$PIPESTATUS` indexed array variable. Migrate `BASH_REMATCH_N` flat variables (from `=~` regex matching) to a proper `BASH_REMATCH` indexed array with capture group support.

### M6.3 — Shopt Options

Add `ShoptOpts` struct to interpreter state and `shopt` builtin. Implement behavioral wiring for: `nullglob` (non-matching globs expand to nothing), `globstar` (`**` matches recursively), `dotglob` (globs include dot-files), `extglob` (extended patterns `+(...)` etc. — parser already enables this), `failglob` (error on no match), `nocaseglob` (case-insensitive glob), `nocasematch` (case-insensitive `[[ =~ ]]` and `case`), `lastpipe` (last pipeline command runs in current shell), `expand_aliases` (enable alias expansion).

### M6.4 — Additional Builtins

Implement missing builtins that AI-generated scripts commonly use:

- `getopts optstring name [args]` — argument parsing with `OPTIND`/`OPTARG`/`OPTERR` state.
- `mapfile`/`readarray` — populate indexed array from stdin. Support `-t` (strip newline), `-d` (delimiter), `-n` (max lines), `-s` (skip lines), `-C` (callback).
- `type [-t|-a|-p] name` — identify whether name is builtin, function, command, or alias. Common pattern: `if type jq &>/dev/null; then ...`.
- `alias name=value` / `unalias name` — define and remove aliases. Requires pre-expansion token rewriting before command execution. Lower priority than other builtins due to architectural complexity with brush-parser.
- `select var in list; do ... done` — menu selection loop. Low priority (interactive feature, rarely used by AI agents), but completes the control-flow set.
- `hash [-r] [name]` — command path caching (can be a no-op that tracks a hash table for compatibility).

### M6.5 — Full `read` Flags

Extend `builtin_read` beyond basic line reading. Add: `-r` (no backslash escaping — already works), `-a arrayname` (read into indexed array — requires M6.1), `-d delim` (read until delimiter instead of newline), `-n count` (read at most N characters), `-N count` (read exactly N characters), `-p prompt` (no-op in sandbox — stdin is pre-provided), `-t timeout` (return failure if stdin empty — sandbox stdin is always fully provided, so this returns immediately).

### M6.6 — Full `declare` Attributes

Extend `builtin_declare` and `Variable` to support all attribute flags: `-i` (integer — arithmetic eval on every assignment), `-l` (lowercase — transform value to lowercase on assignment), `-u` (uppercase — transform to uppercase), `-n` (nameref — variable holds name of another variable, dereference on read/write with depth cap of 10 to prevent loops), `-a` (indexed array), `-A` (associative array). Add attribute bitflags to `Variable` struct to avoid per-variable memory bloat.

### M6.7 — Process Substitution

Implement `<(cmd)` and `>(cmd)`. brush-parser already produces `ProcessSubstitution` AST nodes (interpreter currently returns an error). For `<(cmd)`: execute command, capture stdout, write to temp VFS file (`/tmp/.proc_sub_XXXX`), substitute the temp path into the argument list. For `>(cmd)`: create temp file, after outer command completes read it and pipe to inner command. Clean up temp files with RAII guard. Enables `diff <(sort file1) <(sort file2)` pattern.

### M6.8 — Special Variable Tracking

Several special variables are missing or broken:

- **`$LINENO`** — currently hardcoded to `"0"`. Must track the actual source line number from the AST during execution, updating it at each statement. Critical for error messages and debugging.
- **`$SECONDS`** — elapsed seconds since shell start. Store `Instant::now()` at shell creation, return elapsed on access.
- **`$_`** — last argument of the previous command. Update after each simple command execution.
- **`FUNCNAME`** array — stack of function names during call chain. Push on function entry, pop on return. (Requires M6.1 arrays.)
- **`BASH_SOURCE`** array — stack of source files. Track which file/string each function was defined in.
- **`BASH_LINENO`** array — stack of line numbers where each function call originated.

### M6.9 — Shell Option Enforcement

Several `set`/`shopt` options are parsed but produce no effect:

- **`set -x` (xtrace)** — print each command to stderr before execution, prefixed with `$PS4` (default `"+ "`). Show expanded words, not raw source. Currently the flag is stored but no trace output is generated.
- **`set -v` (verbose)** — print each source line to stderr before parsing/expansion.
- **`set -n` (noexec)** — parse but do not execute commands. Useful for syntax checking.
- **`set -o noclobber`** — prevent `>` from overwriting existing files. Require `>|` to force overwrite.

### M6.10 — Advanced Redirections

Several redirection features that just-bash supports are missing:

- **`exec` builtin** — when invoked with only redirections (`exec > file`, `exec 3< file`), permanently redirect file descriptors for the rest of the shell session. Without redirections, `exec cmd` replaces the shell (in sandbox: just run the command).
- **`/dev/stdin`, `/dev/stdout`, `/dev/stderr`** — special-case these paths in redirection handling (currently only `/dev/null` is handled). `/dev/zero` (reads return empty) and `/dev/full` (writes return ENOSPC) are also useful.
- **FD variable allocation `{varname}>file`** — automatically allocate a file descriptor number, store it in the named variable. Used for advanced I/O multiplexing.
- **Read-write file descriptors `N<>file`** — open file for both reading and writing on FD N.
- **FD movement `N>&M-`** — duplicate FD M to N, then close M.
- **Pipe stderr `|&`** — shorthand for `2>&1 |`, piping both stdout and stderr to next command.

### M6.11 — Parameter Transformation Operators

Implement `${var@operator}` syntax for variable transformations:

- **`${var@Q}`** — quote value for shell reuse (wraps in `$'...'` for control characters).
- **`${var@E}`** — expand backslash escape sequences in value.
- **`${var@A}`** — produce an assignment statement that recreates the variable (e.g., `declare -- var="value"`).
- **`${var@a}`** — return the variable's attribute flags (e.g., `x` for exported, `r` for readonly).

Also add `printf` format specifiers `%b` (interpret backslash escapes) and `%q` (shell-quote output).

---

## Milestone 7: Command Coverage and Discoverability

**Goal**: Fill remaining command gaps identified against just-bash, and add `--help` to every command so AI agents can self-discover usage.

### M7.1 — `--help` Flag for All Commands

Add a `--help` handler to the command dispatch layer (or per-command). When any command receives `--help` as the first argument, print a usage summary to stdout and exit 0. Cover all ~58 existing commands and every new command added in M7. Consider a declarative approach (e.g., a `CommandMeta` struct with name, synopsis, description, options) to avoid per-command boilerplate.

### M7.2 — Core Utility Commands

Implement commonly-used utility commands that AI agents encounter:

- `timeout [-k kill_delay] [-s signal] duration command` — run command with time limit, exit 124 on timeout.
- `time [-p] command` — execute command, report wall-clock time to stderr. `-p` for POSIX format.
- `readlink [-f|-e|-m] path` — resolve symlinks. `-f` canonicalize (all components must exist).
- `rmdir [-p] dir` — remove empty directories. `-p` removes parent directories too.
- `du [-s|-h|-a|-d depth] [path]` — estimate file space usage by walking VFS tree.
- `sha1sum [files]` — SHA-1 hash (add `sha1` crate alongside existing `sha2`).

### M7.3 — Compression and Archiving

Implement archive and compression commands for AI agents working with bundled data:

- `gzip [-d|-c|-k|-f|-r|-1..-9] [files]` — compress files. Via `flate2` crate.
- `gunzip [files]` — decompress (alias for `gzip -d`).
- `zcat [files]` — decompress to stdout (alias for `gzip -dc`).
- `tar [-c|-x|-t|-f archive] [files]` — create, extract, list archives. Support gzip compression (`-z`). Via `tar` crate + `flate2`.

### M7.4 — Binary and File Inspection

Commands for inspecting file contents and types:

- `file [files]` — detect file type via magic bytes + extension mapping.
- `strings [-n min_length] [files]` — extract printable strings from binary data.
- `od [-A addr_format] [-t type] [files]` — octal/hex/decimal dump.
- `split [-l lines|-b bytes] [file [prefix]]` — split file into chunks.

### M7.5 — Search

- `rg [pattern] [path]` — ripgrep-compatible recursive search. Respects `.gitignore`, smart case by default, vimgrep output format. Reuses existing `grep` search infrastructure with ripgrep-style defaults (recursive, smart-case, file-type filters via `-t`/`-T`, glob via `-g`).

### M7.6 — Shell Utility Commands

- `help [command]` — display help for builtins and commands. Can share metadata from M7.1's `--help` infrastructure.
- `clear` — output ANSI clear-screen escape sequence.
- `history` — display command history. Integrates with existing REPL history tracking.

### M7.7 — Default Filesystem Layout and Command Resolution

Currently `RustBashBuilder::build()` creates an empty VFS with only the cwd. `which ls` returns a hardcoded `/usr/bin/ls` without checking the VFS, and that path doesn't exist. Fix:

- **Default filesystem layout**: On build, create `/bin`, `/usr/bin`, `/tmp`, `/home/user` (or `$HOME`), and `/dev` (with `/dev/null`, `/dev/zero`). Match the Unix-like layout AI agents expect.
- **Command stubs**: When commands are registered, write stub files to `/bin/<cmd>` (e.g., `#!/bin/bash\n# built-in: ls`) so they appear in `ls /bin` and VFS existence checks.
- **Default environment variables**: Set sensible defaults for `PATH` (`/usr/bin:/bin`), `HOME`, `USER`, `HOSTNAME`, `OSTYPE`, `SHELL`, `IFS`, `PWD`, `OLDPWD` unless the caller overrides them.
- **Fix `which` command**: Replace hardcoded `REGISTERED_COMMANDS`/`SHELL_BUILTINS` list lookups with actual PATH-based resolution — iterate PATH directories, check VFS file existence, return the real resolved path. Fall back to checking builtins and functions.

### M7.8 — Command Fidelity Infrastructure

Add infrastructure for systematic command correctness:

- **Unknown-flag error handling**: Add a consistent `unknown_option(cmd, flag)` helper that all commands use when encountering unrecognized flags. Return non-zero exit code with a message matching bash format (`cmd: invalid option -- 'x'` / `cmd: unrecognized option '--foo'`).
- **Comparison test suite**: Fixture-based tests that run scripts against real bash and assert matching stdout/stderr/exit code. Record expected output in fixture files for offline replay. Enables differential testing without requiring bash at every `cargo test`.
- **Per-command flag metadata**: Each command exports a declarative flag list (name, type, implemented vs stubbed). Enables coverage tracking and systematic fuzzing of flag combinations.

---

## Milestone 8: Embedded Runtimes and Data Formats

**Goal**: Add embedded language runtimes and data format processing commands, expanding rust-bash from a shell interpreter into a multi-tool sandbox.

### M8.1 — SQLite3 Command

Implement `sqlite3 [database] [query]` via `rusqlite` crate (bundles SQLite as a static library — no external dependency). Support multiple output modes (list, csv, json, column, table, markdown, tabs). Query timeout to prevent runaway queries. Databases stored in VFS as binary blobs. `:memory:` for in-memory databases.

### M8.2 — yq (Multi-Format Data Processor)

Implement `yq` for YAML/XML/TOML/CSV/INI processing with jq-style query syntax. Auto-detect format from file extension, explicit override via `-p input_format -o output_format`. Reuse the existing `jaq` query engine where possible for filter evaluation. Support `-i` for in-place VFS edit. Crates: `serde_yaml`, `quick-xml`, `toml`, `csv`, `rust-ini`.

### M8.3 — xan (CSV Toolkit)

Implement `xan` as a CSV processing toolkit with subcommands: `headers`, `count`, `select`, `search`, `filter`, `sort`, `frequency`, `stats`, `sample`, `slice`, `split`, `cat`, `join`, `flatten`, `transpose`. Translate operations to queries where possible, sharing infrastructure with jq/yq.

### M8.4 — Embedded Python Runtime

Add opt-in `python3`/`python` command. **Design exploration required**: evaluate (a) bundling CPython compiled to WASM (like just-bash), (b) calling host Python via `std::process::Command` behind a feature flag (breaks sandbox but is simpler), or (c) embedding RustPython (pure Rust Python implementation). Option (c) is most aligned with the sandbox model but has stdlib gaps. Feature-gate behind `python` cargo feature.

### M8.5 — Embedded JavaScript Runtime

Add opt-in `js-exec` command. **Design exploration required**: evaluate (a) embedding `boa_engine` (pure Rust JS engine — good sandbox story, limited Node.js compat), (b) `quickjs-rs` bindings (more complete JS, still embeddable), or (c) `deno_core` (V8-based, heavy but full Node.js compat). For AI agent use, basic JS/TS execution with `console.log`, `JSON`, and VFS access is sufficient. Feature-gate behind `javascript` cargo feature.

### M8.6 — html-to-markdown

Implement `html-to-markdown` command for converting HTML to Markdown. Useful for AI agents processing web content fetched via `curl`. Via a Rust HTML-to-Markdown crate (e.g., `htmd` or custom using `scraper` + formatting logic). Support `-b` (bullet character), `-c` (code fence style), heading style selection.

---

## Milestone 9: Platform and API Features

**Goal**: Add platform-level capabilities that make rust-bash a better embeddable runtime for host applications.

### M9.1 — Cooperative Cancellation

Add `Arc<AtomicBool>` cancellation flag to `InterpreterState`. Check in `check_limits()` alongside existing wall-clock timeout. Expose `RustBash::cancel_handle() -> CancelHandle` that the host can call from another thread. This is more ergonomic than the current wall-clock-only approach — hosts get immediate, cooperative cancellation at the next statement boundary.

### M9.2 — Lazy File Loading

Add lazy file materialization to `InMemoryFs`. Files can be registered with a callback (`Box<dyn Fn() -> Result<Vec<u8>, VfsError>>`) instead of upfront content. Callback is invoked on first `read_file`, result is cached. Supports large file sets where most files are never read (e.g., mounting a project directory with thousands of files but only reading a few). Also enables dynamic content generation.

### M9.3 — AST Transform Pipeline

Expose brush-parser AST via a public `parse()` API. Build a `TransformPipeline` that chains visitor plugins over the AST and serializes back to bash script text. Built-in plugins: `CommandCollectorPlugin` (extract unique command names from a script — useful for pre-flight permission checks), `TeePlugin` (inject `tee` to capture per-command stdout). Custom plugin trait for host-defined transforms. Enables script instrumentation without execution.

### M9.4 — Sandbox API Wrapper

Add a high-level `Sandbox` API compatible with the Vercel Sandbox interface: `Sandbox::create(opts)`, `sandbox.run_command(cmd)`, `sandbox.write_files(files)`, `sandbox.read_file(path)`, `sandbox.mkdir(path)`. Wraps `RustBash` with convenience methods and default OverlayFs configuration. Enables drop-in replacement for `@vercel/sandbox` in Rust-based AI agent hosts.

### M9.5 — Virtual /proc Filesystem

Add virtual `/proc/self/status`, `/proc/version`, and `/proc/self/fd/` entries to the VFS. Simulated values only — virtual PID/PPID/UID/GID, synthetic kernel version string. Prevents scripts that probe `/proc` from failing. Mount via `MountableFs` at `/proc`.

### M9.6 — Defense-in-Depth Security Hardening

Formalize security guarantees beyond VFS + NetworkPolicy + ExecutionLimits. Add: fuzz testing suite for parser and interpreter (property-based tests via `proptest` or `arbitrary` crates), resource accounting per-exec (peak memory, total I/O bytes), and documentation of the threat model. Audit all command implementations for potential panics or unbounded allocations.

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

M6.1 (arrays) ─────┬── M6.2 (PIPESTATUS/BASH_REMATCH)
                    ├── M6.4 (mapfile/readarray — needs arrays)
                    ├── M6.5 (read -a — needs arrays)
                    ├── M6.6 (declare -a/-A — needs arrays)
                    └── M6.8 (FUNCNAME/BASH_SOURCE — needs arrays)
M6.3 (shopt) ──────────── (independent — wires into M1.8 globs)
M6.7 (process sub) ────── (independent — parser support exists)
M6.8 (special vars) ───── (independent except FUNCNAME arrays)
M6.9 (set -x/-v/-n) ──── (independent)
M6.10 (adv redirections)  (independent)
M6.11 (param transforms)  (independent)
M7.1 (--help) ─────────── (independent — can start anytime)
M7.2–M7.6 ─────────────── (independent — new command implementations)
M7.7 (default fs layout) ─ (should happen early — affects M7.2+ command testing)
M8.1–M8.3 ─────────────── (depend on M1 command infrastructure)
M8.4–M8.5 ─────────────── (require design exploration — feature-gated)
M9.1–M9.6 ─────────────── (depend on M1–M5 for full platform)
```

**Recommended order (M1–M5)**: M1.1 → M1.2 → M1.3 → M1.4 → M1.5 → M1.6 → M1.7 → M1.8/M1.9/M1.10/M1.11 (parallel) → M1.12 → M1.13 → M1.14 → M1.15 → M3.1 → M2.1 → M2.2 → M2.3 → M2.4 → M4.1 → M5.1 → M5.2 → M5.3

**Recommended order (M6–M9)**: M6.1 (arrays — critical path) → M6.8 ($LINENO/$SECONDS — quick wins) → M6.2/M6.3 (parallel) → M6.4/M6.5/M6.6 (parallel, unlocked by arrays) → M6.9/M6.10/M6.11 (parallel) → M6.7 → M7.7 (default fs layout — do before other M7 work) → M7.1 → M7.2/M7.3/M7.4/M7.5/M7.6/M7.8 (parallel) → M8.1 → M8.2 → M8.3 → M8.4/M8.5 (design exploration first) → M9.1 → M9.2 → M9.3 → M9.4 → M9.5 → M9.6

---

## Open Questions

1. **Adapter layer for brush-parser types?** Wrapping AST types insulates from upstream changes but adds code. Currently not implemented — we use brush-parser types directly.

2. **Async vs sync API**: `exec()` is synchronous. An async wrapper can be added later if needed for timeout or concurrent pipe execution. Timeouts are currently implemented via wall-clock checks during execution.

3. **Error message compatibility**: Currently matching bash error format (`cmd: msg`) but not exact wording. Close enough for AI agent usage.

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation | Status |
|------|-----------|--------|------------|--------|
| brush-parser breaking changes | Medium | Medium | Pin to crates.io version (`0.3.0`); update test suite on upgrade | Open |
| awk/sed complexity explosion | Low | Medium | 80/20 subset implemented and shipped | ✅ Resolved |
| Word expansion edge cases | Medium | High | Differential testing against real bash | Open |
| WASM binary size too large | Medium | Medium | Feature-gate heavy commands (planned for M5) | Open |
| Command substitution refactoring | Low | Medium | Interior mutability approach implemented | ✅ Resolved |
| Lock poisoning from panics | Low | High | parking_lot::RwLock (non-poisoning) implemented | ✅ Resolved |
| Array edge case complexity | Medium | High | Bash arrays have many subtle behaviors (sparse indexing, quoting differences between `@` and `*`, unset-without-reindex). Differential testing against real bash required. | Open |
| brush-parser array AST representation | Low | Medium | Verify how brush-parser represents `${arr[0]}`, `${arr[@]:1:3}`, `${!arr[@]}` before starting M6.1. Parser already handles `MemberKeys` and `ArrayElementName`. | Open |
| Nameref infinite loops | Low | Medium | `declare -n ref=ref` or circular chains. Cap resolution depth at 10. | Open |
| Embedded runtime binary size | Medium | High | Python/JS runtimes (M8.4/M8.5) could bloat binary significantly. Feature-gate behind cargo features. | Open |
| Process substitution temp file leaks | Low | Low | Temp VFS files from `<(cmd)` could leak on panic. Use cleanup-on-drop guard. | Open |
