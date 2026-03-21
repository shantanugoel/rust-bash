# Chapter 4: Interpreter Engine

## Overview

The interpreter is the core execution engine. It walks the AST produced by brush-parser, expands words, manages control flow, dispatches commands, handles pipelines and redirections, and enforces execution limits. It is the largest and most complex component of rust-bash.

## Execution Entry Point

```rust
// Called by RustBash::exec()
pub fn execute_program(
    program: &Program,
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError>
```

The interpreter receives a parsed `Program` (a list of compound lists) and a mutable reference to the interpreter state. It executes each compound list in sequence, accumulating stdout and stderr, and returns the final result.

## InterpreterState

```rust
pub struct InterpreterState {
    pub fs: Arc<dyn VirtualFs>,
    pub env: HashMap<String, Variable>,
    pub cwd: String,
    pub functions: HashMap<String, FunctionDef>,
    pub last_exit_code: i32,
    pub commands: HashMap<String, Box<dyn VirtualCommand>>,
    pub shell_opts: ShellOpts,
    pub limits: ExecutionLimits,
    pub counters: ExecutionCounters,
    pub network_policy: NetworkPolicy,
    pub positional_params: Vec<String>,
    pub shell_name: String,
    // Internal fields (pub(crate)):
    // should_exit, loop_depth, control_flow, random_seed,
    // local_scopes, in_function_depth, traps, in_trap,
    // errexit_suppressed
}
```

The state is persistent across `exec()` calls — VFS contents, environment variables, function definitions, traps, and cwd all carry over. Only stdout/stderr buffers and execution counters reset per call.

The interpreter is split across several submodules:
- **`walker.rs`** — AST walking, pipeline execution, redirections, compound commands, function calls, subshells
- **`expansion.rs`** — Word expansion (parameter, command substitution, tilde, glob, word splitting)
- **`arithmetic.rs`** — Arithmetic expression evaluator for `$((...))`, `let`, `((...))` with full operator support
- **`brace.rs`** — Brace expansion (`{a,b,c}` and `{1..10..2}`)
- **`builtins.rs`** — Shell builtins that modify interpreter state (cd, export, set, trap, etc.)
- **`pattern.rs`** — Glob pattern matching used by case statements and pathname expansion

## AST Walking

The interpreter walks the AST in a recursive descent pattern:

```
execute_program(Program)
  └── for each CompoundList:
      execute_compound_list(CompoundList)
        └── for each CompoundListItem:
            execute_and_or_list(AndOrList)
              └── execute_pipeline(Pipeline)
                  └── for each Command in pipeline:
                      execute_command(Command)
                        ├── Simple → execute_simple_command()
                        ├── Compound → execute_compound_command()
                        └── Function → store in state.functions
```

### Compound List Execution

A compound list is a sequence of items separated by `;` or `&`. All items execute in order. **Stdout and stderr accumulate across all items** — `echo a; echo b` outputs `"a\nb\n"`. Only the exit code comes from the last item.

### And-Or List Execution

`&&` and `||` short-circuit based on the exit code of the left side:
- `cmd1 && cmd2` — execute cmd2 only if cmd1 succeeds (exit code 0)
- `cmd1 || cmd2` — execute cmd2 only if cmd1 fails (exit code ≠ 0)

### Pipeline Execution

A pipeline connects multiple commands with `|`. Each command's stdout becomes the next command's stdin.

**Current approach**: sequential execution with buffered stdout between stages. Each command runs to completion, then its stdout is fed as stdin to the next command. This is simpler than concurrent pipe execution and sufficient for the data sizes AI agents work with.

**Future optimization**: for large data pipelines, implement concurrent execution where commands run simultaneously with streaming pipes between them.

The `!` prefix on a pipeline negates its exit code.

## Word Expansion

Word expansion is the process of transforming raw word text into final strings. It follows bash's expansion order:

1. **Brace expansion** — `{a,b,c}` → three separate words
2. **Tilde expansion** — `~` → `$HOME`
3. **Parameter expansion** — `$VAR`, `${VAR:-default}`, `${VAR%pattern}`, etc.
4. **Command substitution** — `$(cmd)` → execute cmd, capture stdout
5. **Arithmetic expansion** — `$((1+2))` → `3`
6. **Word splitting** — unquoted results split on `$IFS` (default: space, tab, newline)
7. **Glob expansion** — unquoted wildcards matched against VFS

### Parameter Expansion

The interpreter evaluates these `brush_parser::word::WordPiece` variants:

| Expansion | Syntax | Behavior |
|-----------|--------|----------|
| Simple | `$VAR`, `${VAR}` | Value of VAR, or empty string |
| Default value | `${VAR:-word}` | Value of VAR, or `word` if unset/empty |
| Assign default | `${VAR:=word}` | Like `:-` but also assigns |
| Error if unset | `${VAR:?msg}` | Error with `msg` if unset/empty |
| Use alternative | `${VAR:+word}` | `word` if VAR is set, else empty |
| String length | `${#VAR}` | Length of value |
| Suffix removal | `${VAR%pattern}`, `${VAR%%pattern}` | Remove shortest/longest suffix match |
| Prefix removal | `${VAR#pattern}`, `${VAR##pattern}` | Remove shortest/longest prefix match |
| Substitution | `${VAR/pattern/string}` | Replace first/all matches |
| Substring | `${VAR:offset:length}` | Substring extraction |
| Case modification | `${VAR^}`, `${VAR,}` | Uppercase/lowercase first or all |
| Array element | `${arr[N]}` | Value at index N |
| All elements | `${arr[@]}`, `${arr[*]}` | All values; `[@]` separate words, `[*]` joined by IFS |
| Array length | `${#arr[@]}` | Number of elements |
| Array keys | `${!arr[@]}` | All indices/keys |

### Variables

Variables use the `Variable` struct with `VariableValue` (Scalar, IndexedArray, or AssociativeArray) and `VariableAttrs` bitflags (EXPORTED, READONLY, etc.). Scalars accessed with `[0]` return their value. Arrays are sparse (BTreeMap-backed) — `unset arr[N]` removes an element without reindexing. `declare -a` creates indexed arrays; `declare -A` creates associative arrays.

### Special Variables

| Variable | Value |
|----------|-------|
| `$?` | Exit code of last command |
| `$#` | Number of positional parameters |
| `$@` | All positional parameters (separate words in double quotes) |
| `$*` | All positional parameters (single word joined by IFS in double quotes) |
| `$0` | Name of the script/shell |
| `$1`–`$9`, `${10}`+ | Positional parameters |
| `$$` | Process ID (synthetic — returns a fixed value) |
| `$!` | PID of last background command (not applicable — always empty) |
| `$RANDOM` | Random integer 0–32767 |
| `$LINENO` | Current line number (best-effort) |

### Command Substitution

`$(cmd)` and backtick substitution execute the inner command string by recursively invoking the interpreter, capturing stdout, and stripping trailing newlines. The `VirtualFs` trait uses interior mutability (`Arc<parking_lot::RwLock<…>>`), so command substitution can share the filesystem naturally. For subshells and `$(...)`, the interpreter deep-clones the VFS so mutations don't leak back to the parent scope.

### Word Splitting

After expansion, unquoted results are split on characters in `$IFS` (default: space, tab, newline). Double-quoted expansions are *not* word-split — `"$VAR"` always produces exactly one word even if VAR contains spaces.

### Glob Expansion

After word splitting, unquoted words containing `*`, `?`, or `[...]` are expanded against the VFS. The glob is resolved relative to the current working directory. If no files match, the pattern is left as-is (bash default behavior without `failglob`).

## Compound Commands

### If/Elif/Else

```bash
if condition_list; then
    body
elif condition_list; then
    body
else
    body
fi
```

The interpreter evaluates each condition list. If the exit code is 0, the corresponding body executes. Otherwise, the next elif/else branch is tried.

### For Loop

```bash
for var in word_list; do
    body
done
```

The word list is expanded (including word splitting and glob expansion). The loop body executes once per resulting word, with `var` set to each word. Iteration count is checked against `max_loop_iterations`.

### While/Until Loop

```bash
while condition_list; do body; done
until condition_list; do body; done
```

`while` loops while condition succeeds; `until` loops while condition fails. Both check iteration limits.

### Case Statement

```bash
case $word in
    pattern1) body1 ;;
    pattern2|pattern3) body2 ;;
    *) default_body ;;
esac
```

The word is expanded, then matched against each pattern using glob matching. `;;` terminates the case, `;&` falls through to the next body, `;;&` continues pattern testing.

### Brace Group and Subshell

- `{ cmd1; cmd2; }` — executes in the current shell (shares state)
- `( cmd1; cmd2 )` — executes in a subshell (cloned state, changes don't propagate back)

## Function Definitions and Calls

```bash
func_name() {
    local var=$1
    echo "hello $var"
    return 0
}
func_name "world"
```

Functions are stored in `state.functions`. When called:
1. Positional parameters (`$1`, `$2`, etc.) are set from the call arguments
2. `local` creates function-scoped variables that restore previous values on return
3. `return N` exits the function with exit code N
4. Function lookup: special builtins → functions → registered commands (see "Command Resolution Order" section below)

## Redirections

Redirections are applied per-command. The interpreter:
1. Saves the current stdout/stderr/stdin state
2. Applies redirections in order (left to right)
3. Executes the command with the modified I/O
4. Restores the original I/O state

For file redirections (`>`, `>>`, `<`), the target path is expanded and resolved against the VFS.

For fd duplication (`2>&1`), the target fd's output is redirected to the source fd's destination.

## Command Resolution Order

When the interpreter encounters a command name, it resolves in this order:

1. **Special shell builtins** — `cd`, `export`, `exit`, `set`, `local`, `return`, `break`, `continue`, `eval`, `source`, `read`, `trap`, `shift`, `unset`, `declare`, `readonly`, `let`, `:`. Handled directly by the interpreter because they modify interpreter state. These cannot be shadowed by functions.
2. **User-defined functions** — stored in `InterpreterState.functions`. Functions *can* shadow registered commands (e.g., a function named `echo` overrides the built-in echo).
3. **Registered commands** — looked up in `InterpreterState.commands` HashMap.
4. **"Command not found"** — stderr error, exit code 127.

> **Note**: In real bash, regular builtins (like `echo`, `test`) *can* be shadowed by functions. Our order matches this — only special builtins (those that modify state) are unshadowable. This is a deliberate safety choice: preventing functions from shadowing `cd`, `exit`, or `export` avoids accidental breakage in agent-generated scripts.

External process execution is impossible by design — there is no fallback to `std::process::Command`.

## Shell Builtins

These commands are handled directly by the interpreter (not the command registry) because they modify interpreter state:

| Builtin | Effect |
|---------|--------|
| `cd` | Changes `state.cwd` |
| `export` | Marks variable as exported |
| `unset` | Removes variable |
| `set` | Sets shell options (`-e`, `-u`, `-o pipefail`, `-x`) |
| `shift` | Shifts positional parameters |
| `local` | Declares function-scoped variable |
| `declare` | Declares variable with attributes |
| `readonly` | Makes variable read-only |
| `return` | Returns from function |
| `exit` | Exits with code |
| `break`/`continue` | Loop control flow |
| `eval` | Parse and execute string |
| `source`/`.` | Parse and execute file |
| `read` | Read line from stdin into variable |
| `trap` | Register exit/error handler (only `trap EXIT` and `trap ERR` are meaningful; signal-based traps are no-ops in a sandbox) |
| `let` | Evaluate arithmetic expression |
| `:` | No-op (always succeeds) |

## Control Flow Signals

`break`, `continue`, and `return` use a signal mechanism (enum variant or special result type) that propagates up through nested execution to the correct loop or function level. `break N` and `continue N` support optional numeric arguments for breaking out of nested loops.

## Shell Options Enforcement

The `ShellOpts` struct tracks shell options set via `set` builtin:

### `set -e` (errexit)

When enabled, the shell exits immediately when a command returns a non-zero exit code. Exceptions (matching bash behavior):

- Commands in `if`/`while`/`until` conditions
- Left side of `&&`/`||` chains
- Negated commands (`! cmd`)
- Commands in subshells (subshell may exit, but parent only sees exit code)

Implementation uses an `errexit_suppressed` counter on `InterpreterState` to track exception context nesting.

### `set -u` (nounset)

Errors on expansion of unset variables. Exceptions:

- `${VAR:-default}` and other default/alternative value expansions
- Special variables (`$@`, `$*`, `$#`, `$?`, `$-`, etc.)

### `set -o pipefail`

Pipeline exit code becomes the rightmost non-zero exit code (instead of just the last command's exit code). E.g., `false | true` returns 1 instead of 0.
