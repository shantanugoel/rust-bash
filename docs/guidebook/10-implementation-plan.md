# Chapter 10: Implementation Plan

## Milestones Overview

| # | Milestone | Goal |
|---|-----------|------|
| M1 | Core Shell | Production interpreter + VFS trait + ~35 commands |
| M2 | Text Processing | awk, sed, jq, diff + remaining text commands |
| M3 | Execution Safety | Limits enforcement, network policy |
| M4 | Filesystem Backends | OverlayFs, ReadWriteFs, MountableFs |
| M5 | Integration | C FFI, WASM, CLI binary, AI SDK wrapper |
| M6 | Shell Language Completeness | Arrays, shopt, process substitution, special vars, advanced redirections, missing builtins, differential testing |
| M7 | Command Coverage & Discoverability | Missing commands, `--help` for all commands, AI agent docs, agent workflow tests |
| M8 | Embedded Runtimes & Data Formats | Python, JavaScript, SQLite, yq, xan, runtime boundary hardening |
| M9 | Platform, Security & Execution API | Cancellation, lazy files, AST transforms, sandbox API, fuzz testing, threat model, binary encoding, network enhancements, VFS fidelity |

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

Add `ExecutionLimits` + `ExecutionCounters` to state. Check limits at command dispatch, function calls, loop iterations, output appends, and wall-clock time. Return structured `LimitExceeded` errors. Additional limits to add: `maxSourceDepth` (default 100 — prevents `source` nesting stack overflow), `maxFileDescriptors` (default 1024 — prevents FD exhaustion from `exec 3<file` loops).

### M3.2 — Network Access Control ✅

Implement `NetworkPolicy`. Sandboxed `curl` validates URL against allow-list before HTTP request. Method restrictions, redirect following, response size limits. Additional security features to add: **DNS rebinding / SSRF protection** — `denyPrivateRanges: bool` option that DNS-resolves the URL hostname *before* the HTTP request and rejects private IP ranges (10.x, 172.16.x, 192.168.x, 127.x, ::1, link-local); pin resolved IP to the connection to prevent TOCTOU attacks. **Request transforms / credential brokering** — per-allowed-URL `transform` callback that can inject headers (auth tokens) at the fetch boundary so secrets never enter the sandbox environment. This enables secure API access without exposing credentials to scripts.

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

### M5.3 — WASM Target ✅

`wasm32-unknown-unknown` + `wasm-bindgen`. JavaScript wrapper. npm package (`@rust-bash/core`) with TypeScript types, dual-entry (Node.js + browser), WASM backend with `initWasm()` / `createWasmBackend()`.

**Design decision:** Separate `wasm-bindgen` (browser) and planned napi-rs (Node.js native addon) builds, unified under a single `@rust-bash/core` package with conditional exports. The package auto-detects the environment: `tryLoadNative()` for Node.js, `initWasm()` for browsers.

### M5.4 — AI SDK Integration ✅

Framework-agnostic tool definitions (JSON Schema + handler functions) exported from the npm package. MCP server mode for the CLI binary (`rust-bash --mcp`). Documented recipe-based adapters for Vercel AI SDK, LangChain.js, OpenAI API, and Anthropic API. The core exports `bashToolDefinition` (JSON Schema), `createBashToolHandler()`, `formatToolForProvider()`, and `handleToolCall()` — the universal building blocks that work with any AI agent framework. Framework-specific adapters are thin (~10-line) wrappers documented as recipes, not hard dependencies.

**Design decisions:**
- **Unified package:** Single `@rust-bash/core` package with native Node.js addon as primary backend and WASM as automatic fallback for browsers/edge runtimes.
- **Custom commands:** `defineCommand()` API in TypeScript mirrors the Rust `VirtualCommand` trait. Custom commands are registered at `Bash.create()` time and participate in pipelines and redirections.
- **Tool primitives:** `bashToolDefinition` + `formatToolForProvider('openai' | 'anthropic' | 'mcp')` for zero-dependency provider formatting. `handleToolCall()` dispatcher supports `bash`, `readFile`, `writeFile`, `listDirectory` tool names.

---

## Milestone 6: Shell Language Completeness

**Goal**: Close remaining bash language gaps so AI-generated scripts that use arrays, shopt, advanced builtins, and process substitution work without modification.

### M6.1 — Indexed and Associative Arrays ✅

Implemented `VariableValue` enum (`Scalar`/`IndexedArray(BTreeMap<usize, String>)`/`AssociativeArray(BTreeMap<String, String>)`), `VariableAttrs` bitflags, array assignment/expansion/arithmetic, `declare -a`/`-A`, `unset arr[n]`, `${arr[@]}`, `${arr[*]}`, `${#arr[@]}`, `${!arr[@]}`, array `+=()` append, and `maxArrayElements` execution limit. 31 integration tests.

**Why first in M6**: Arrays are the critical path — `$PIPESTATUS`, `BASH_REMATCH`, `mapfile`, `read -a`, and `declare -A` all depend on this.

### M6.2 — `$PIPESTATUS` and `BASH_REMATCH` as Arrays ✅

Expose the `exit_codes` vector already collected in `execute_pipeline` as the `$PIPESTATUS` indexed array variable. Migrate `BASH_REMATCH_N` flat variables (from `=~` regex matching) to a proper `BASH_REMATCH` indexed array with capture group support.

### M6.3 — Shopt Options ✅

Add `ShoptOpts` struct to interpreter state and `shopt` builtin. Implement behavioral wiring for: `nullglob` (non-matching globs expand to nothing), `globstar` (`**` matches recursively), `dotglob` (globs include dot-files), `extglob` (extended patterns `+(...)` etc. — parser already enables this), `failglob` (error on no match), `nocaseglob` (case-insensitive glob), `nocasematch` (case-insensitive `[[ =~ ]]` and `case`), `lastpipe` (last pipeline command runs in current shell — requires changing pipeline execution to avoid subshell for the final command when enabled), `expand_aliases` (enable alias expansion), `xpg_echo` (make `echo` interpret backslash escapes by default, like `echo -e`), `globskipdots` (don't match `.` and `..` with glob patterns — bash 5.2+ default).

### M6.4 — Additional Builtins ✅

Implement missing builtins that AI-generated scripts commonly use:

- ✅ `getopts optstring name [args]` — argument parsing with `OPTIND`/`OPTARG`/`OPTERR` state.
- ✅ `mapfile`/`readarray` — populate indexed array from stdin. Support `-t` (strip newline), `-d` (delimiter), `-n` (max lines), `-s` (skip lines), `-C` (callback).
- ✅ `type [-t|-a|-p] name` — identify whether name is builtin, function, command, or alias. Common pattern: `if type jq &>/dev/null; then ...`.
- ✅ `command [-pVv] name` — run command bypassing functions, or describe a command. `command -v git` is the most common tool-detection pattern in bash. `-p` uses default PATH.
- ✅ `builtin name [args]` — force execution of a builtin, bypassing same-named functions.
- ✅ `pushd [-n] [dir | +N | -N]` / `popd [-n] [+N | -N]` / `dirs [-clpv] [+N | -N]` — full directory stack. ~260 lines in just-bash. Very common in build scripts and CI.
- ✅ `alias name=value` / `unalias name` — define and remove aliases. Requires pre-expansion token rewriting before command execution. Lower priority than other builtins due to architectural complexity with brush-parser.
- `select var in list; do ... done` — menu selection loop. Low priority (interactive feature, rarely used by AI agents), but completes the control-flow set. Blocked: brush-parser 0.3.0 has no `Select` variant in `CompoundCommand`.
- ✅ `hash [-r] [name]` — command path caching with real hash table. Maintain `HashMap<String, PathBuf>` in interpreter state. `hash name` resolves and caches the PATH lookup; subsequent invocations skip PATH search. `hash -r` clears the table. `hash` with no args lists cached entries. just-bash implements this with a real `hashTable: Map`; matching that behavior avoids silent divergence in scripts that use `hash -r` to force re-resolution after PATH changes.
- ✅ `wait [pid|jobspec]` — no-op stub that returns 0 immediately. Prevents scripts from failing when they include `wait`.

### M6.5 — Full `read` Flags ✅

Extend `builtin_read` beyond basic line reading. Add: `-r` (no backslash escaping — already works), `-a arrayname` (read into indexed array — requires M6.1), `-d delim` (read until delimiter instead of newline), `-n count` (read at most N characters), `-N count` (read exactly N characters), `-p prompt` (no-op in sandbox — stdin is pre-provided), `-t timeout` (return failure if stdin empty — sandbox stdin is always fully provided, so this returns immediately).

### M6.6 — Full `declare` Attributes ✅

Extend `builtin_declare` and `Variable` to support all attribute flags: `-i` (integer — arithmetic eval on every assignment), `-l` (lowercase — transform value to lowercase on assignment), `-u` (uppercase — transform to uppercase), `-n` (nameref — variable holds name of another variable, dereference on read/write with depth cap of 10 to prevent loops), `-a` (indexed array), `-A` (associative array). Add attribute bitflags to `Variable` struct to avoid per-variable memory bloat.

### M6.7 — Process Substitution ✅

Implemented `<(cmd)` and `>(cmd)`. brush-parser already produces `ProcessSubstitution` AST nodes. For `<(cmd)`: execute command, capture stdout, write to temp VFS file (`/tmp/.proc_sub_N`), substitute the temp path into the argument list. For `>(cmd)`: create temp file, after outer command completes read it and pipe to inner command. Temp files cleaned up after enclosing command completes. Enables `diff <(sort file1) <(sort file2)` pattern.

### M6.8 — Special Variable Tracking ✅

Several special variables are missing or broken:

- **`$LINENO`** — currently hardcoded to `"0"`. Must track the actual source line number from the AST during execution, updating it at each statement. Critical for error messages and debugging.
- **`$SECONDS`** — elapsed seconds since shell start. Store `Instant::now()` at shell creation, return elapsed on access.
- **`$_`** — last argument of the previous command. Update after each simple command execution.
- **`FUNCNAME`** array — stack of function names during call chain. Push on function entry, pop on return. (Requires M6.1 arrays.)
- **`BASH_SOURCE`** array — stack of source files. Track which file/string each function was defined in.
- **`BASH_LINENO`** array — stack of line numbers where each function call originated.
- **`$PPID`** — virtual parent PID. Return configurable value (default 1). Referenced in process-aware scripts.
- **`$UID` / `$EUID`** — virtual user ID and effective user ID. `if [ "$EUID" -ne 0 ]; then` is an extremely common pattern. Return configurable value (default 1000).
- **`$BASHPID`** — current PID. Unlike `$$`, changes in subshells. Track separately via subshell nesting counter.
- **`SHELLOPTS` / `BASHOPTS`** — readonly colon-separated lists of currently enabled `set` and `shopt` options respectively. Dynamically maintained — scripts check `[[ $SHELLOPTS =~ errexit ]]`. Must update on every `set -o`/`shopt -s` change.
- **`$MACHTYPE` / `$HOSTTYPE`** — machine description strings. Can be set to static values (e.g., `x86_64-pc-linux-gnu`, `x86_64`).

### M6.9 — Shell Option Enforcement ✅

The following `set`/`shopt` options are now enforced:

- **`set -x` (xtrace)** — traces simple commands and bare assignments to stderr, prefixed with `$PS4` (default `"+ "`). Shows expanded words. Trace state is captured before dispatch so `set +x` is traced but `set -x` is not.
- **`set -v` (verbose)** — accepted and stored; behavioral effect (echoing source lines) not yet implemented (requires line-by-line parse-execute loop).
- **`set -n` (noexec)** — skips all commands except `set` itself, enabling syntax checking.
- **`set -C` / `set -o noclobber`** — prevents `>` and `&>` from overwriting existing files; `>|` forces overwrite; `>>` and `&>>` are unaffected. `/dev/null` is always allowed.
- **`set -a` (allexport)** — marks variables as EXPORTED on assignment in `set_variable()`. Other assignment sites (`declare X` without value, `read -a`) are not yet wired.
- **`set -f` (noglob)** — disables glob expansion entirely in `glob_expand_words()`.
- **`set -o posix`** — accepted and stored as a no-op stub.
- **`set -o vi` / `set -o emacs`** — accepted as no-ops (tracked in options). Not meaningful in a sandbox.

### M6.10 — Advanced Redirections ✅

All six redirection features are now implemented:

- ✅ **`exec` builtin** — when invoked with only redirections (`exec > file`, `exec 3< file`), permanently redirect file descriptors for the rest of the shell session. Without redirections, `exec cmd` replaces the shell (in sandbox: just run the command).
- ✅ **`/dev/stdin`, `/dev/stdout`, `/dev/stderr`** — special-cased in redirection handling alongside `/dev/null`. `/dev/zero` (reads return null bytes) and `/dev/full` (writes return ENOSPC) are also supported.
- ✅ **FD variable allocation `{varname}>file`** — automatically allocate a file descriptor number (starting at 10), store it in the named variable. `exec {fd}>&-` closes it.
- ✅ **Read-write file descriptors `N<>file`** — open file for both reading and writing on FD N.
- ✅ **FD movement `N>&M-`** — duplicate FD M to N, then close M.
- ✅ **Pipe stderr `|&`** — shorthand for `2>&1 |`, piping both stdout and stderr to next command.

### M6.11 — Parameter Transformation Operators ✅

Implement `${var@operator}` syntax for variable transformations:

- **`${var@Q}`** — quote value for shell reuse (wraps in `$'...'` for control characters).
- **`${var@E}`** — expand backslash escape sequences in value.
- **`${var@P}`** — expand prompt escape sequences (`\u`, `\h`, `\w`, `\d`, `\t`, `\[`, `\]`, ANSI colors). Used by PS1/PS4 expansion.
- **`${var@A}`** — produce an assignment statement that recreates the variable (e.g., `declare -- var="value"`).
- **`${var@a}`** — return the variable's attribute flags (e.g., `x` for exported, `r` for readonly).
- **`${!ref}` (indirect expansion)** — dereference variable whose name is stored in `ref`. Handle `${!ref}` pointing to arrays, slicing via indirection. Different from namerefs (M6.6) — this is a read-time expansion. Widely used for dynamic variable access.
- **`${!prefix*}` / `${!prefix@}` (variable name expansion)** — expand to all variable names matching the given prefix. Used for iterating config variables (e.g., `${!DOCKER_*}`).
- **`printf -v varname`** — assign formatted output to a variable instead of stdout. Very common pattern to avoid subshell overhead: `printf -v hex '%02x' 255`.

Also add `printf` format specifiers `%b` (interpret backslash escapes) and `%q` (shell-quote output).

### M6.12 — Differential Testing Against Real Bash ✅

Fixture-based comparison test suite that records expected output from real `/bin/bash` and replays it against rust-bash on every `cargo test`. Delivered: 269 comparison test cases across 34 fixture files covering shell language features (quoting, expansion, word splitting, globbing, redirections, pipes, control flow, functions, here-documents, arrays, PIPESTATUS, BASH_REMATCH, declare attributes, read flags, parameter transforms, special variables, set options, advanced redirections) plus 188 spec test cases across 14 fixture files for `grep`, `sed`, `awk`, and `jq`, plus 2,274 Oils spec test cases across 142 files imported from the upstream Oils project (commit `7789e21d81537a5b47bacbd4267edf7c659a9366`, Apache 2.0). Recording mode (`RECORD_FIXTURES=1`) re-captures ground truth from real bash. Infrastructure uses `datatest-stable` for per-file test discovery and `toml_edit` for round-trip fixture updates. The Oils suite uses a pass-list approach (everything defaults to xfail, pass-list tracks known passes). Combined test surface: **2,731 cases** across three suites. Of the comparison cases, 263 pass, 5 are xfail, and 1 is skip. Of the Oils cases, 802 pass, 1,393 are xfail (product gaps), and 79 are skip (harness limitations); 42 files are skipped entirely (non-applicable, CLI-only, process/trap). All runners use a three-state model (pass/xfail/skip) and treat unexpected passes as failures to force fixture promotion.

---

## Milestone 7: Command Coverage and Discoverability

**Goal**: Fill remaining command gaps identified against just-bash, and add `--help` to every command so AI agents can self-discover usage.

### ✅ M7.1 — `--help` Flag for All Commands

Add a `--help` handler to the command dispatch layer (or per-command). When any command receives `--help` as the first argument, print a usage summary to stdout and exit 0. Cover all ~58 existing commands and every new command added in M7. Consider a declarative approach (e.g., a `CommandMeta` struct with name, synopsis, description, options) to avoid per-command boilerplate.

### ✅ M7.2 — Core Utility Commands

Implement commonly-used utility commands that AI agents encounter:

- ✅ `timeout [-k kill_delay] [-s signal] duration command` — run command with time limit, exit 124 on timeout.
- ✅ `time [-p] pipeline` — shell keyword (not just a command) that wraps an entire pipeline with timing; report wall-clock, user, and system time to stderr. `-p` for POSIX format. Must handle `time cmd1 | cmd2` as a single timed unit.
- ✅ `readlink [-f|-e|-m] path` — resolve symlinks. `-f` canonicalize (all components must exist).
- ✅ `rmdir [-p] dir` — remove empty directories. `-p` removes parent directories too.
- ✅ `du [-s|-h|-a|-d depth] [path]` — estimate file space usage by walking VFS tree.
- ✅ `sha1sum [files]` — SHA-1 hash (add `sha1` crate alongside existing `sha2`).
- ✅ `fgrep` / `egrep` — register as aliases for `grep -F` / `grep -E`. Deprecated but widely used in existing scripts.
- ✅ `sh [-c command]` — alias for `bash`. Run a subshell. `sh -c "..."` is very common.
- ✅ `bc [-l]` — arbitrary precision calculator. Basic arithmetic, comparison, and `scale` support. Covers `echo "1.5 * 3" | bc` pattern.

### ✅ M7.3 — Compression and Archiving

Implement archive and compression commands for AI agents working with bundled data:

- ✅ `gzip [-d|-c|-k|-f|-r|-1..-9] [files]` — compress files. Via `flate2` crate.
- ✅ `gunzip [files]` — decompress (alias for `gzip -d`).
- ✅ `zcat [files]` — decompress to stdout (alias for `gzip -dc`).
- ✅ `tar [-c|-x|-t|-f archive] [files]` — create, extract, list archives. Support gzip compression (`-z`). Via `tar` crate + `flate2`.

**Binary data path**: `CommandResult.stdout_bytes` and `CommandContext.stdin_bytes` carry `Vec<u8>` through pipelines without UTF-8 conversion. `InterpreterState.pipe_stdin_bytes` propagates binary between pipeline stages.

### ✅ M7.4 — Binary and File Inspection

Commands for inspecting file contents and types:

- ✅ `file [files]` — detect file type via magic bytes + extension mapping.
- ✅ `strings [-n min_length] [files]` — extract printable strings from binary data.
- `od [-A addr_format] [-t type] [files]` — octal/hex/decimal dump.
- ✅ `split [-l lines|-b bytes] [file [prefix]]` — split file into chunks.

### ✅ M7.5 — Search

- ✅ `rg [pattern] [path]` — ripgrep-compatible recursive search. Respects `.gitignore`, smart case by default, vimgrep output format. Reuses existing `grep` search infrastructure with ripgrep-style defaults (recursive, smart-case, file-type filters via `-t`/`-T`, glob via `-g`).

### ✅ M7.6 — Shell Utility Commands

- ✅ `help [command]` — display help for builtins and commands. Comprehensive built-in help database (just-bash has 650+ lines of help text). Can share metadata from M7.1's `--help` infrastructure.
- ✅ `clear` — output ANSI clear-screen escape sequence.
- ✅ `history` — display command history. Integrates with existing REPL history tracking.

### ✅ M7.7 — Default Filesystem Layout and Command Resolution

Currently `RustBashBuilder::build()` creates an empty VFS with only the cwd. `which ls` returns a hardcoded `/usr/bin/ls` without checking the VFS, and that path doesn't exist. Fix:

- **Default filesystem layout**: On build, create `/bin`, `/usr/bin`, `/tmp`, `/home/user` (or `$HOME`), and `/dev` (with `/dev/null`, `/dev/zero`). Match the Unix-like layout AI agents expect.
- **Command stubs**: When commands are registered, write stub files to `/bin/<cmd>` (e.g., `#!/bin/bash\n# built-in: ls`) so they appear in `ls /bin` and VFS existence checks.
- **Default environment variables**: Set sensible defaults for `PATH` (`/usr/bin:/bin`), `HOME`, `USER`, `HOSTNAME`, `OSTYPE` (`linux-gnu`), `MACHTYPE` (`x86_64-pc-linux-gnu`), `HOSTTYPE` (`x86_64`), `SHELL` (`/bin/bash`), `BASH` (`/bin/bash`), `BASH_VERSION`, `IFS`, `PWD`, `OLDPWD`, `TERM` (`xterm-256color`) unless the caller overrides them.
- **Fix `which` command**: Replace hardcoded `REGISTERED_COMMANDS`/`SHELL_BUILTINS` list lookups with actual PATH-based resolution — iterate PATH directories, check VFS file existence, return the real resolved path. Fall back to checking builtins and functions.

### ✅ M7.8 — Command Fidelity Infrastructure

Add infrastructure for systematic command correctness:

- ✅ **Unknown-flag error handling**: Add a consistent `unknown_option(cmd, flag)` helper that all commands use when encountering unrecognized flags. Return non-zero exit code with a message matching bash format (`cmd: invalid option -- 'x'` / `cmd: unrecognized option '--foo'`).
- ✅ **Path-argument fidelity for file-oriented commands**: Add shared conformance tests (and helper utilities where useful) for commands that receive shell-expanded path operands. Mixed file/directory operand sets must follow bash/GNU tool behavior per command instead of failing uniformly on the first directory or treating every operand as the same kind of path. Examples: `ls *` should list regular files directly while also listing directory operands correctly, and `grep pattern *` should still process file operands while reporting directory operands in non-recursive mode.
- ✅ **Comparison test suite**: Fixture-based tests that run scripts against real bash and assert matching stdout/stderr/exit code. Record expected output in fixture files for offline replay. Enables differential testing without requiring bash at every `cargo test`.
- ✅ **Per-command flag metadata**: Each command exports a declarative flag list (name, type, implemented vs stubbed). Enables coverage tracking and systematic fuzzing of flag combinations.

### M7.9 — AI Agent Documentation (`AGENTS.md`) ✅

Ship a purpose-built `AGENTS.md` in the npm package and alongside the CLI binary. This is the primary interface documentation for AI agents consuming rust-bash. Inspired by just-bash's `AGENTS.npm.md` which ships as `dist/AGENTS.md`.

- ✅ **Content**: Quick-start examples, available commands grouped by category, tools-by-file-format recipes (JSON with `jq`, YAML with `yq`, CSV with `xan`), key behavioral notes (isolation model, no real filesystem, no network by default).
- ✅ **Distribution**: Include in npm package (`@rust-bash/core`), embed in CLI `--help`, and publish to docs site.
- ✅ **Validation**: Add a test that verifies all documented commands actually exist in the registry and all code examples parse successfully.

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

### M8.7 — Runtime Boundary Hardening

When embedded runtimes (Python, JavaScript) and FFI/WASM boundaries are introduced, add defense-in-depth measures at each boundary crossing. Inspired by just-bash's `DefenseInDepthBox` system (AsyncLocalStorage-based context tracking, monkey-patching dangerous globals, violation logging), but adapted to Rust's capabilities:

- **Capability-based isolation**: Embedded runtimes (M8.4/M8.5) receive only the capabilities explicitly granted — VFS access, environment variables, network policy. No ambient authority leaks through the runtime boundary.
- **Context propagation**: Track execution context across async/FFI boundaries. Ensure that cancellation signals, execution limits, and network policy enforcement propagate correctly when commands cross runtime boundaries (e.g., `js-exec` calling back into the shell via `exec`).
- **WASM boundary audit**: Verify that the `wasm-bindgen` boundary in M5.3 does not expose host filesystem, environment variables, or network access beyond what is explicitly granted. Document the attack surface of the WASM boundary.
- **FFI boundary audit**: Verify that the C FFI boundary in M5.2 does not allow memory corruption, use-after-free, or double-free via the exported API. Document all unsafe invariants.

**Why in M8**: This work becomes concrete only when embedded runtimes exist. The security design should be done alongside the runtime implementations, not retrofitted.

---

## Milestone 9: Platform, Security & Execution API

**Goal**: Add platform-level capabilities that make rust-bash a better embeddable runtime for host applications.

### M9.1 — Cooperative Cancellation

Add `Arc<AtomicBool>` cancellation flag to `InterpreterState`. Check in `check_limits()` alongside existing wall-clock timeout. Expose `RustBash::cancel_handle() -> CancelHandle` that the host can call from another thread. This is more ergonomic than the current wall-clock-only approach — hosts get immediate, cooperative cancellation at the next statement boundary.

Additionally, support **per-exec cancellation** via an optional `signal` parameter on `exec()`. just-bash accepts `AbortSignal` per-exec call, enabling agent orchestrators to cancel individual commands without destroying the entire shell instance. In Rust, this can be an `Option<Arc<AtomicBool>>` on `ExecOptions` that overrides the instance-level cancel handle for that execution only. This is important for timeout-per-command patterns in agent loops.

### M9.2 — Lazy File Loading

Add lazy file materialization to `InMemoryFs`. Files can be registered with a callback (`Box<dyn Fn() -> Result<Vec<u8>, VfsError>>`) instead of upfront content. Callback is invoked on first `read_file`, result is cached. Supports large file sets where most files are never read (e.g., mounting a project directory with thousands of files but only reading a few). Also enables dynamic content generation.

### M9.3 — AST Transform Pipeline

Expose brush-parser AST via a public `parse()` API. Build a `TransformPipeline` that chains visitor plugins over the AST and serializes back to bash script text. Built-in plugins: `CommandCollectorPlugin` (extract unique command names from a script — useful for pre-flight permission checks), `TeePlugin` (inject `tee` to capture per-command stdout). Custom plugin trait for host-defined transforms. Enables script instrumentation without execution.

### M9.4 — High-Level Convenience API

Add high-level convenience features to the `Bash` class (TypeScript) and `RustBashBuilder` (Rust): command filtering, per-exec env/cwd isolation, logger interface, virtual process info, safe argument passing, script normalization. These enrich the existing API rather than introducing a separate `Sandbox` class.

Additional API features:
- **Command filtering** — `commands: Vec<CommandName>` option restricts which commands are available per-session. Critical for least-privilege sandboxing (e.g., prevent `curl`, `rm -rf /`). just-bash has this as `commands?: CommandName[]` in `BashOptions`.
- **Per-exec env/cwd isolation** — `exec()` accepts `env`, `cwd`, `replace_env` overrides that are restored after execution. Useful for multi-tenant scenarios. The `replace_env: bool` option starts execution with an empty environment (only the provided env vars), rather than merging. This is important for reproducibility and isolation — just-bash supports this as `replaceEnv`.
- **ExecResult environment snapshot** — Return the post-execution environment as `env: HashMap<String, String>` in `ExecResult`. Critical for AI agent frameworks that need to inspect env changes after a script runs (e.g., `source .env` then read the vars). just-bash includes this as `env: Record<string, string>` in every exec result.
- **Logger interface** — `BashLogger` trait with `info`/`debug` methods for execution tracing (xtrace, command dispatch, etc.). Essential for debugging in production.
- **Trace/performance profiling** — `TraceCallback` for per-command timing events with category, name, duration, and details. Enables performance analysis of scripts. just-bash has `TraceEvent` with `{ category, name, durationMs, details }`. Can be implemented as an extension of the logger or a separate callback.
- **Virtual process info** — configurable `ProcessInfo { pid, ppid, uid, gid }` in builder options. Powers `$$`, `$PPID`, `$UID`, `$EUID`, `$BASHPID`, and `/proc/self/status`. Supports multi-sandbox scenarios with unique PIDs. just-bash wires this through constructor options as `processInfo`.
- **Safe argument passing** — `ExecOptions.args: Vec<String>` for additional argv entries that bypass shell parsing entirely (no escaping/splitting/globbing). Like `child_process.spawn(cmd, args)`. Safe way to pass filenames with special characters.
- **Script normalization** — strip leading whitespace from template literals while preserving heredoc content. `raw_script: bool` option to disable.

### M9.5 — Virtual /proc Filesystem

Add virtual `/proc/self/status`, `/proc/version`, `/proc/self/fd/`, and `/proc/self/environ` entries to the VFS. Simulated values only — virtual PID/PPID/UID/GID (from M9.4 ProcessInfo), synthetic kernel version string. Prevents scripts that probe `/proc` from failing. Mount via `MountableFs` at `/proc`.

### M9.6 — Defense-in-Depth Security Hardening

Formalize security guarantees beyond VFS + NetworkPolicy + ExecutionLimits. This milestone covers both runtime hardening and documentation.

**Runtime hardening:**
- **Resource accounting per-exec** — track peak memory, total I/O bytes, and command counts. Return as optional metrics in `ExecResult` for observability.
- **ReDoS audit** — verify that all user-provided regex paths (`=~`, `grep`, `sed`) use Rust's `regex` crate (RE2-based, linear-time by default) and that no `fancy-regex` or PCRE paths are exposed to untrusted input.
- **Exported env isolation** — audit that non-exported shell variables do not leak to child commands — only exported variables should be visible.
- **Panic audit** — audit all command implementations for potential panics or unbounded allocations. Every command must handle invalid input gracefully.

**Threat model documentation:**
Write a comprehensive `THREAT_MODEL.md` (inspired by just-bash's 400-line threat model) covering:
- **Threat actors**: Untrusted script author (primary), malicious data source, compromised dependency.
- **Trust boundaries**: Script input → parser, interpreter → VFS, interpreter → commands, interpreter → network, FFI/WASM boundaries.
- **Trust assumptions**: What is trusted (host application, Rust runtime, OS kernel) and what is not (scripts, data, network responses).
- **Attack surface analysis**: For each boundary, enumerate attack vectors and existing mitigations.
- **Residual risks**: Known gaps, accepted risks, and their mitigations.
- **Security properties**: What the sandbox guarantees (no host FS access, no process spawning, no network without policy) and what it does not.

### M9.7 — Fuzz Testing Suite

Dedicated fuzz testing infrastructure for the interpreter and all command implementations. This is critical for a sandbox that runs untrusted code — fuzzing finds crashes, panics, infinite loops, and resource exhaustion that unit tests miss.

- **Parser/interpreter fuzzing** — use `cargo-fuzz` (libfuzzer) to generate random bash scripts and feed them to the interpreter. Targets: parse-only (find parser panics), parse-and-execute (find interpreter panics), expansion (find expansion edge cases). Seed corpus from existing test scripts and real-world bash snippets.
- **Command fuzzing** — fuzz individual commands with random argument combinations and random stdin. Prioritize commands that do text processing (`sed`, `awk`, `grep`, `jq`) and file manipulation (`tar`, `gzip`).
- **Property-based testing** — use `proptest` or `arbitrary` crates for structured fuzzing with invariant checks: (a) no command should panic regardless of input, (b) execution limits should never be exceeded without returning `LimitExceeded`, (c) VFS operations should never corrupt internal state, (d) every command should produce valid UTF-8 or controlled binary output.
- **Differential fuzzing** — generate random scripts, run in both rust-bash and real bash, compare stdout/stderr/exit code. Flag divergences for investigation. Builds on M7.8 comparison infrastructure.
- **Continuous integration** — configure fuzz targets to run in CI with a time budget (e.g., 5 minutes per target per run). Store crash artifacts in `fuzz/artifacts/` for regression testing.

### M9.8 — Binary Data and Output Encoding Model

Design and implement a systematic approach to binary data flow through the shell pipeline. This is a prerequisite for M7.3 (compression/archiving) and affects the exec() output boundary.

- **Pipeline byte transparency**: Audit and ensure that pipe data flows as `Vec<u8>` through the entire pipeline path (command stdout → pipe → next command stdin). Verify that no intermediate step lossy-converts to `String`. Rust's `Vec<u8>` is naturally correct, but the pipe/redirect/capture paths must be audited.
- **Output boundary encoding**: At the `exec()` return boundary, decide how to handle non-UTF-8 output. Options: (a) return `Vec<u8>` for stdout/stderr (breaking API change), (b) return `String` with lossy replacement and a separate `stdout_bytes: Option<Vec<u8>>` field, (c) add an `encoding_hint: Option<String>` field to `ExecResult` indicating binary content (like just-bash's `stdoutEncoding?: "binary"`).
- **Input boundary**: Ensure `stdin` can carry binary data. Currently `stdin` is `&str` — consider `Option<&[u8]>` for binary stdin support.
- Revisit and complete M7.3 fully after this task is completed

just-bash solves this with latin1 strings internally (each char = one byte) and a `decodeBinaryToUtf8()` function at the output boundary. Rust should leverage `Vec<u8>` naturally but must design the API boundary carefully.

### M9.9 — Network Policy Enhancements

Extend `NetworkPolicy` with features from just-bash's battle-tested network layer:

- **`dangerously_allow_all: bool`** — convenience bypass for development/trusted environments. Clearly named to discourage production use. just-bash has `dangerouslyAllowFullInternetAccess`.
- **`deny_private_ranges: bool`** — reject URLs that resolve to private/loopback IP addresses (10.x, 172.16.x, 192.168.x, 127.x, ::1, link-local). Performs both lexical hostname checks and DNS resolution to catch DNS rebinding attacks. Enforced even when `dangerously_allow_all` is true. Uses resolved-IP pinning to prevent TOCTOU attacks.
- **Request transforms / credential brokering** — per-allowed-URL `transform` callback that can inject headers (auth tokens) at the fetch boundary so secrets never enter the sandbox environment. just-bash has `RequestTransform { headers: Record<string, string> }` per `AllowedUrl` entry. This enables secure API access without exposing credentials to scripts.
- **Response size limits** — `max_response_size: usize` to prevent memory exhaustion from large HTTP responses.

### M9.10 — VFS Fidelity Enhancements

Add VFS trait methods for better compatibility with real-world shell scripts:

- **`utimes(path, atime, mtime)`** — set file access and modification times. Required for `touch -t` and scripts that rely on file timestamps for logic (e.g., Makefiles, caching). just-bash's `IFileSystem` includes this.
- **`/dev/stdin`, `/dev/stdout`, `/dev/stderr`** — special-case these paths in I/O handling. Currently only `/dev/null` is handled (M6.10 mentions these but they should also be part of the VFS layer).

### M9.11 — Agent Workflow Integration Tests

Build a test suite simulating realistic AI agent workflows, inspired by just-bash's 13 `agent-examples/*.test.ts` files. Each test represents a real-world agent task:

- **Bug investigation**: grep through logs, analyze stack traces, identify root causes.
- **Code review**: diff files, check for patterns, analyze dependencies.
- **Codebase exploration**: find files, read configs, navigate directory structures.
- **Log analysis**: parse structured logs with awk/jq, aggregate statistics.
- **Text processing workflows**: multi-stage pipelines combining sed, awk, sort, uniq.
- **Config analysis**: read JSON/YAML configs, extract values, validate structure.
- **Security audit**: grep for secrets, check file permissions, analyze network configs.

These tests validate the command surface against realistic agent patterns, not just shell correctness. They also serve as living documentation of expected usage.

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
M6.12 (diff tests) ────── (independent — start early for confidence)
M7.1 (--help) ─────────── (independent — can start anytime)
M7.2–M7.6 ─────────────── (independent — new command implementations)
M7.7 (default fs layout) ─ (should happen early — affects M7.2+ command testing)
M7.9 (AGENTS.md) ──────── (after M7.1–M7.6 — needs command list to document)
M7.10 (agent tests) ───── (after M7.2+ — needs commands to test workflows)
M8.1–M8.3 ─────────────── (depend on M1 command infrastructure)
M8.4–M8.5 ─────────────── (require design exploration — feature-gated)
M8.7 (runtime hardening)   (alongside M8.4/M8.5 — design with runtimes)
M9.1–M9.5 ─────────────── (depend on M1–M5 for full platform)
M9.6 (security hardening)  (independent — can start threat model early)
M9.7 (fuzz testing) ───── (depends on M7.8 comparison infra; can start early with interpreter-only targets)
M9.8 (binary encoding) ── (before M7.3 compression — prerequisite for byte transparency)
M9.9 (network enhancements) (extends M3.2 — independent)
M9.10 (VFS fidelity) ──── (independent)
```

**Recommended order (M1–M5)**: M1.1 → M1.2 → M1.3 → M1.4 → M1.5 → M1.6 → M1.7 → M1.8/M1.9/M1.10/M1.11 (parallel) → M1.12 → M1.13 → M1.14 → M1.15 → M3.1 → M2.1 → M2.2 → M2.3 → M2.4 → M4.1 → M5.1 → M5.2 → M5.3

**Recommended order (M6–M9)**: M6.12 (diff tests — start early for confidence) → M6.1 (arrays — critical path) → M6.8 ($LINENO/$SECONDS — quick wins) → M6.2/M6.3 (parallel) → M6.4/M6.5/M6.6 (parallel, unlocked by arrays) → M6.9/M6.10/M6.11 (parallel) → M6.7 → M9.8 (binary encoding — prerequisite for compression) → M7.7 (default fs layout — do before other M7 work) → M7.1 → M7.2/M7.3/M7.4/M7.5/M7.6/M7.8 (parallel) → M7.9/M7.10 (agent docs & tests) → M8.1 → M8.2 → M8.3 → M8.4/M8.5 (design exploration first) → M8.7 (runtime hardening, alongside M8.4/M8.5) → M9.1 → M9.2 → M9.3 → M9.4 → M9.5 → M9.6 (threat model can start earlier) → M9.7 → M9.9/M9.10 (parallel)

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
| Binary data corruption in pipes | Medium | High | If any pipe/redirect/capture path converts `Vec<u8>` to `String`, binary data (gzip, tar) will be silently corrupted. Audit pipeline data flow before M7.3. | Open |
| `set -a` assignment site coverage | Medium | Medium | Allexport must be wired into every assignment site (plain, declare, local, for, read). Missing one causes silent env leaks. Track with exhaustive tests. | Open |
| Indirect expansion complexity | Medium | Medium | `${!ref}` when `ref` points to array elements, namerefs, or chained indirection — interactions are subtle. Budget extra testing time. | Open |
| SSRF TOCTOU in DNS resolution | Low | High | DNS rebinding: hostname must be resolved and IP pinned before the HTTP connection to prevent time-of-check/time-of-use attacks. Use custom resolver + connect-time IP pinning. | Open |
| ExecResult API design for binary output | Medium | Medium | If stdout is `String`, binary data from gzip/tar is lost. If `Vec<u8>`, ergonomics suffer for text-heavy AI agent use. Need careful design in M9.8 — possibly dual fields or encoding hint. | Open |
| M9 scope creep | Medium | Medium | M9 now covers security, execution API, networking, VFS fidelity, and fuzz testing. Risk of becoming a dumping ground. Mitigate by explicitly splitting into subsections and prioritizing P0 items. | Open |
| Runtime boundary capability leaks | Low | High | Embedded runtimes (M8.4/M8.5) could accidentally inherit host capabilities. Design capability-based isolation from the start (M8.7). | Open |
| Missing threat model during early adoption | Medium | Medium | Sandbox providers may adopt rust-bash before M9.6 threat model is written. Ship a minimal threat model early as part of README/guidebook. | Open |
