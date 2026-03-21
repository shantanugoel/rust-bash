# Oils Spec Test Import Plan

## Background

### What is Oils?

[Oils](https://github.com/oils-for-unix/oils) (formerly Oil Shell) is an open-source Unix shell
project that aims to be a better bash. As part of building their bash-compatible `osh` interpreter,
the Oils team created the most comprehensive open-source bash conformance test suite in existence:
**2,728 test cases across 136 `.test.sh` files**.

The tests are licensed under Apache 2.0. [just-bash](https://github.com/nicholasgasior/just-bash)
imported this corpus in December 2025 and runs it via a custom TypeScript parser + Vitest harness.

### Test format

Oils spec tests use a simple plain-text format:

```bash
#### test name
echo hello
## stdout: hello

#### multiline output
echo line1
echo line2
## STDOUT:
line1
line2
## END

#### expected failure
false
## status: 1
```

Key format elements:
- `#### name` — test case delimiter and name
- `## stdout: value` — single-line expected stdout
- `## STDOUT:\n...\n## END` — multiline expected stdout
- `## STDERR:\n...\n## END` — multiline expected stderr
- `## stderr-json: "..."` — expected stderr (JSON-encoded)
- `## status: N` — expected exit code (default 0)
- `## OK bash stdout/status/...` — acceptable alternate output for bash
- `## BUG bash stdout/status/...` — known bash bug output
- `## N-I bash stdout/status/...` — shell-specific variant ("Not Implemented in bash")
- File-level headers: `## compare_shells:`, `## tags:`, `## oils_failures_allowed:`

### Current test landscape

| Metric | just-bash | rust-bash | Gap |
|---|---|---|---|
| Comparison fixture files | 32 | 34 | rust-bash slightly broader |
| Comparison fixture cases | 532 | 269 | −263 (depth gap) |
| Oils spec files | 136 | 0 | −136 |
| Oils spec cases | 2,728 | 0 | −2,728 |
| **Total test surface** | **~3,260** | **269** | **−2,991** |

After importing the Oils corpus, rust-bash would have **~2,997 cases** — near parity.

---

## Oils file inventory (136 files, 2,728 cases)

Grouped by feature area and mapped approximately to rust-bash milestones.

> [!NOTE]
> This section is a planning aid, not an exact milestone ledger. Several Oils files span more
> than one rust-bash milestone — for example `builtin-set.test.sh` touches both M1.15 and M6.9,
> and `builtin-printf.test.sh` mixes M1.14 coverage with a smaller M6.11 tail. The file counts
> are exact; the milestone labels are approximate unless explicitly called out as mixed.

### Core shell — M1 (est. ~70–80% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `word-split.test.sh` | 55 | M1.3 | Word splitting, IFS |
| `brace-expansion.test.sh` | 55 | M1.9 | `{a,b}`, `{1..10}` |
| `builtin-bracket.test.sh` | 52 | M1.6 | `[` / `test` builtin |
| `dbracket.test.sh` | 49 | M1.6 | `[[ ]]` compound command |
| `assign.test.sh` | 47 | M1 | Variable assignment basics |
| `vars-special.test.sh` | 42 | M1/M6.8 | `$?`, `$$`, `$!`, etc. |
| `var-sub-quote.test.sh` | 41 | M1.3 | Quoted variable substitution |
| `builtin-vars.test.sh` | 41 | M1/M6 | `unset`, `readonly`, `export` |
| `glob.test.sh` | 39 | M1.8 | Pathname expansion |
| `assign-extended.test.sh` | 39 | M1 | Extended assignment forms |
| `shell-grammar.test.sh` | 38 | M1 | Parser edge cases |
| `var-op-test.test.sh` | 37 | M1.4 | `${x:-default}`, `${x:+alt}` |
| `here-doc.test.sh` | 36 | M1.10 | Here-documents |
| `quote.test.sh` | 35 | M1.3 | Quoting mechanics |
| `errexit.test.sh` | 35 | M1.15 | `set -e` behavior |
| `command-sub.test.sh` | 30 | M1.4 | `$(...)` command substitution |
| `var-op-strip.test.sh` | 29 | M1.4 | `${x#pat}`, `${x##pat}`, `${x%pat}` |
| `loop.test.sh` | 29 | M1.7 | `for`, `while`, `until` |
| `extglob-match.test.sh` | 29 | M1.8 | Extended glob matching |
| `var-op-patsub.test.sh` | 28 | M1.4 | `${x/pat/rep}` |
| `pipeline.test.sh` | 26 | M1 | Pipelines |
| `builtin-echo.test.sh` | 25 | M1.14 | `echo` builtin |
| `extglob-files.test.sh` | 23 | M1.8 | Extended globs on filesystem |
| `builtin-eval-source.test.sh` | 22 | M1.5 | `eval` and `source` |
| `append.test.sh` | 20 | M1 | `+=` append operator |
| `smoke.test.sh` | 18 | M1 | Basic smoke tests |
| `arith.test.sh` | 74 | M1.11 | Arithmetic expansion |
| `arith-context.test.sh` | 16 | M1.11 | `(( ))` arithmetic context |
| `arith-dynamic.test.sh` | 4 | M1.11 | Dynamic arithmetic cases |
| `dparen.test.sh` | 15 | M1.11 | Double-paren parsing |
| `func-parsing.test.sh` | 15 | M1.12 | Function definition parsing |
| `tilde.test.sh` | 14 | M1 | Tilde expansion |
| `case_.test.sh` | 13 | M1.13 | `case` statement |
| `sh-func.test.sh` | 12 | M1.12 | Shell functions |
| `exit-status.test.sh` | 11 | M1.15 | Exit status propagation |
| `var-op-len.test.sh` | 9 | M1.4 | `${#x}` length |
| `for-expr.test.sh` | 9 | M1.7 | C-style `for ((...))` |
| `word-eval.test.sh` | 8 | M1.3 | Word evaluation |
| `glob-bash.test.sh` | 8 | M1.8 | Bash-specific glob |
| `bool-parse.test.sh` | 8 | M1.6 | Boolean expression parsing |
| `var-op-slice.test.sh` | 22 | M1.4 | `${x:offset:length}` |
| `var-sub.test.sh` | 6 | M1.4 | Variable substitution |
| `var-num.test.sh` | 7 | M1 | `$1`, `$#`, `$@` |
| `if_.test.sh` | 5 | M1.6 | `if` statement |
| `command-parsing.test.sh` | 5 | M1 | Command parsing |
| `paren-ambiguity.test.sh` | 9 | M1 | Parser ambiguity around parentheses |
| `command-sub-ksh.test.sh` | 4 | M1.4 | Ksh-style command sub |
| `temp-binding.test.sh` | 4 | M1 | `VAR=val cmd` |
| `empty-bodies.test.sh` | 3 | M1 | Empty function/loop bodies |
| `comments.test.sh` | 2 | M1 | Comment handling |
| `subshell.test.sh` | 2 | M1 | `(...)` subshells |
| `let.test.sh` | 2 | M1.11 | `let` builtin |
| `whitespace.test.sh` | 5 | M1 | Whitespace handling |
| **Subtotal** | **1,212** | | |

### Arrays — M6.1/M6.2 (est. ~60% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `array.test.sh` | 77 | M6.1 | Comprehensive array tests |
| `array-assoc.test.sh` | 42 | M6.1 | Associative arrays |
| `array-sparse.test.sh` | 40 | M6.1 | Sparse array behavior |
| `array-literal.test.sh` | 19 | M6.1 | Array literal syntax |
| `array-compat.test.sh` | 12 | M6.1 | Cross-shell compat |
| `array-assign.test.sh` | 11 | M6.1 | Array assignment forms |
| `array-basic.test.sh` | 5 | M6.1 | Array basics |
| **Subtotal** | **206** | | |

### Builtins and shell state — mixed M1/M6 (est. ~20–40% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `builtin-read.test.sh` | 64 | M6.5 | `read` builtin flags |
| `builtin-printf.test.sh` | 63 | M1.14 / M6.11 | Mostly core `printf`; a smaller tail covers bash-only additions |
| `alias.test.sh` | 48 | M6.4 | Alias expansion |
| `builtin-type-bash.test.sh` | 31 | M6.4 | `type` builtin (bash) |
| `builtin-getopts.test.sh` | 31 | M6.4 | `getopts` builtin |
| `builtin-set.test.sh` | 24 | M1.15 / M6.9 | Core `set -e/-u/-o pipefail` plus later option work |
| `builtin-cd.test.sh` | 30 | M1 | `cd` builtin |
| `builtin-meta.test.sh` | 18 | M6.4 | `command`, `builtin` |
| `builtin-dirs.test.sh` | 18 | M6.4 | `pushd`/`popd`/`dirs` |
| `command_.test.sh` | 16 | M6.4 | `command` builtin |
| `builtin-special.test.sh` | 12 | mixed M1/M6 | Special builtins |
| `builtin-meta-assign.test.sh` | 11 | M6.4 | `local`, `declare` in meta |
| `builtin-misc.test.sh` | 7 | mixed M1/M6 | Miscellaneous builtins |
| `builtin-type.test.sh` | 6 | M6.4 | `type` builtin |
| **Subtotal** | **379** | | |

### Redirections — M6.10 (est. ~40% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `redirect.test.sh` | 41 | M6.10 | Basic redirections |
| `redirect-command.test.sh` | 23 | M6.10 | Redirections on commands |
| `redirect-multi.test.sh` | 13 | M6.10 | Multiple redirections |
| `redir-order.test.sh` | 5 | M6.10 | Redirection ordering |
| **Subtotal** | **82** | | |

### Shell options — M6.3/M6.9 (est. ~25% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `sh-options.test.sh` | 39 | M6.3/M6.9 | Shell options |
| `sh-options-bash.test.sh` | 9 | M6.3/M6.9 | Bash-specific options |
| `strict-options.test.sh` | 17 | M6.9 | Strict mode options |
| `xtrace.test.sh` | 19 | M6.9 | `set -x` tracing |
| **Subtotal** | **84** | | |

### Variable and regex features — mixed M1/M6 (est. ~40% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `nameref.test.sh` | 32 | M6.6 | Namerefs (`declare -n`) |
| `var-ref.test.sh` | 31 | M6.11 | `${!ref}` indirect refs |
| `var-op-bash.test.sh` | 27 | M6.11 | Bash-specific var ops |
| `regex.test.sh` | 37 | M1.6 / M6.2 | `[[ =~ ]]` plus `BASH_REMATCH` |
| `vars-bash.test.sh` | 1 | M6.8 | Bash-specific special variables |
| **Subtotal** | **128** | | |

### Process substitution — M6.7 (est. ~0–10% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `process-sub.test.sh` | 9 | M6.7 | `<(...)` / `>(...)` |
| **Subtotal** | **9** | | |

### Shell process / trap features outside the initial `exec()` harness

These files are useful reference material, but they do not map cleanly onto chapter 10 today.
Most are either explicit non-goals for the interpreter (`&`, `jobs`, signal fidelity) or would
need a dedicated CLI/process harness rather than the existing `RustBash::exec()` path.

| File | Cases | Why skipped initially |
|---|---|---|
| `background.test.sh` | 27 | Background execution and job control are explicit non-goals in chapter 1 |
| `builtin-process.test.sh` | 28 | Requires a fuller process model than the current interpreter exposes |
| `builtin-kill.test.sh` | 20 | Depends on real signal/process semantics rather than sandboxed execution |
| `builtin-trap.test.sh` | 33 | Only minimal trap support is planned today; full trap fidelity is not milestoned |
| `builtin-trap-bash.test.sh` | 23 | Bash-specific trap semantics beyond the current roadmap |
| `builtin-trap-err.test.sh` | 23 | `trap ERR` semantics are more detailed than the current roadmap promises |
| **Subtotal** | **154** | |

### CLI / REPL features (separate harness, not the initial `exec()` import)

These are meaningful for the CLI binary and REPL experience, but not for the first Oils import.
If we want them later, they should go through a dedicated CLI harness rather than the interpreter
test runner that powers `comparison.rs` and `spec_tests.rs`.

| File | Cases | Why skipped initially |
|---|---|---|
| `interactive.test.sh` | 18 | Requires interactive session behavior |
| `interactive-parse.test.sh` | 1 | Requires interactive parser state |
| `builtin-completion.test.sh` | 51 | Completion system is not part of `RustBash::exec()` |
| `builtin-history.test.sh` | 17 | REPL history belongs to the CLI integration surface |
| `builtin-fc.test.sh` | 14 | Editor/history workflow, not interpreter execution |
| `builtin-bind.test.sh` | 9 | Readline binding behavior is CLI-only |
| `builtin-times.test.sh` | 1 | Better validated through a CLI-oriented harness if we add it |
| `prompt.test.sh` | 33 | Prompt expansion is REPL/CLI behavior, not `exec()` behavior |
| **Subtotal** | **144** | |

### Cross-cutting / meta (est. ~40% pass rate)

| File | Cases | Milestone | Notes |
|---|---|---|---|
| `bugs.test.sh` | 28 | mixed | Known bugs in shells |
| `parse-errors.test.sh` | 27 | M1 | Parser error handling |
| `fatal-errors.test.sh` | 5 | M1 | Fatal error handling |
| `introspect.test.sh` | 13 | M6.8 | Shell introspection |
| `globignore.test.sh` | 18 | M6.3 | GLOBIGNORE shopt |
| `globstar.test.sh` | 5 | M6.3 | `shopt -s globstar` |
| `nocasematch-match.test.sh` | 6 | M6.3 | Case-insensitive match |
| `unicode.test.sh` | 7 | M1 | Unicode handling |
| `nul-bytes.test.sh` | 16 | M1 | NUL byte handling |
| `serialize.test.sh` | 10 | mixed | Serialization forms |
| `builtin-bash.test.sh` | 13 | M6.4 | Bash-specific builtins |
| `sh-usage.test.sh` | 17 | M1 | Shell usage/invocation |
| **Subtotal** | **165** | | |

### Non-applicable (skip entirely)

| File | Cases | Reason |
|---|---|---|
| `zsh-assoc.test.sh` | 7 | Zsh-specific |
| `zsh-idioms.test.sh` | 3 | Zsh-specific |
| `ble-idioms.test.sh` | 26 | Ble.sh-specific |
| `ble-features.test.sh` | 9 | Ble.sh-specific |
| `ble-unset.test.sh` | 5 | Ble.sh-specific |
| `nix-idioms.test.sh` | 6 | Nix-specific |
| `toysh.test.sh` | 8 | Toybox shell |
| `toysh-posix.test.sh` | 23 | Toybox POSIX tests |
| `blog1.test.sh` | 9 | Blog examples (non-bash) |
| `blog2.test.sh` | 8 | Blog examples |
| `blog-other1.test.sh` | 6 | Blog examples |
| `explore-parsing.test.sh` | 5 | Oils parser exploration |
| `print-source-code.test.sh` | 4 | Oils-specific |
| `spec-harness-bug.test.sh` | 1 | Harness meta-test |
| `posix.test.sh` | 15 | POSIX-only (not bash) |
| `shell-bugs.test.sh` | 1 | Meta |
| `known-differences.test.sh` | 2 | Meta |
| `divergence.test.sh` | 4 | Cross-shell divergence |
| `type-compat.test.sh` | 7 | Cross-shell type compat |
| `assign-dialects.test.sh` | 4 | Cross-shell dialects |
| `assign-deferred.test.sh` | 9 | Oils-specific deferred |
| `arg-parse.test.sh` | 3 | Oils arg parsing |
| **Subtotal** | **165** | |

---

## Projected pass rates after import

These are rough planning estimates, not measured results. The actual baseline should be established
by the first import run and then written back into the guidebook.

| Category | Files | Cases | Est. pass | Est. xfail | Est. skip |
|---|---|---|---|---|---|
| Core shell (M1) | 53 | 1,212 | ~848 | ~364 | — |
| Arrays (M6.1/M6.2) | 7 | 206 | ~125 | ~81 | — |
| Builtins and shell state (mixed M1/M6) | 14 | 379 | ~95 | ~284 | — |
| Redirections (M6.10) | 4 | 82 | ~33 | ~49 | — |
| Shell options (M6.3/M6.9) | 4 | 84 | ~21 | ~63 | — |
| Variable and regex features (mixed M1/M6) | 5 | 128 | ~51 | ~77 | — |
| Process substitution (M6.7) | 1 | 9 | ~0 | ~9 | — |
| Cross-cutting | 12 | 165 | ~66 | ~99 | — |
| Shell process / trap features skipped initially | 6 | 154 | — | — | 154 |
| CLI / REPL features skipped initially | 8 | 144 | — | — | 144 |
| Non-applicable | 22 | 165 | — | — | 165 |
| **Total** | **136** | **2,728** | **~1,239** | **~1,026** | **463** |

**Initial overall pass rate: ~55% of runnable cases (1,239 / 2,265)**

### Combined test surface after import

| Metric | just-bash | rust-bash | Delta |
|---|---|---|---|
| Comparison fixtures | 532 cases | 269 cases | −263 |
| Oils spec tests | 2,728 cases | 2,728 cases | 0 |
| **Total** | **~3,260** | **~2,997** | **−263** |

The depth gap in comparison fixtures (−263) is minor. The real gap is **implementation coverage**
— as features land, Oils cases flip from xfail to pass automatically.

> [!NOTE]
> This comparison is intentionally "like-for-like" shell-conformance surface area. It excludes
> rust-bash's 200 command-spec cases for `grep`, `sed`, `awk`, and `jq`, because just-bash does
> not have a directly comparable suite for those commands.

---

## Implementation plan

### Phase 1: Parser and harness (~200–300 lines Rust)

**Goal:** Parse Oils `.test.sh` format and run cases through the existing test infrastructure.

#### 1.1 Oils format parser

Create `tests/common/oils_format.rs`:

```rust
pub struct OilsTestCase {
    pub name: String,
    pub code: String,
    pub expected_stdout: Option<String>,
    pub expected_stderr: Option<String>,
    pub expected_status: i32, // default 0
    pub bash_expected_stdout: Option<String>,
    pub bash_expected_stderr: Option<String>,
    pub bash_expected_status: Option<i32>,
}

pub struct OilsTestFile {
    pub cases: Vec<OilsTestCase>,
    pub tags: Vec<String>,
}

pub fn parse_oils_file(content: &str) -> OilsTestFile { ... }
```

Parser logic:
1. Split on `^#### (.+)$` — each section is one test case
2. Separate code lines from `## ` metadata lines
3. Handle `## stdout: value` (single-line) and `## STDOUT:\n...\n## END` (multiline)
4. Handle `## STDERR:\n...\n## END` and `## stderr-json:` for expected stderr
5. Handle `## status: N` (default 0)
6. Handle bash-specific overrides for stdout/stderr/status (`OK`, `BUG`, and `N-I` forms)
7. Skip file-level headers (`## compare_shells:`, `## tags:`, `## oils_failures_allowed:`)

#### 1.2 Test harness

Create `tests/oils_spec.rs`:

```rust
fn run_oils_spec_file(path: &Path) {
    let content = fs::read_to_string(path).unwrap();
    let test_file = parse_oils_file(&content);

    for case in &test_file.cases {
        // Check skip list (per-file or per-case)
        // Check xfail list
        // Run through RustBashBuilder::new().exec(&case.code)
        // Compare stdout, stderr, exit status, preferring bash_expected_* when present
        // Report pass/xfail/unexpected-pass/fail
    }
}
```

`## N-I bash ...` metadata should be treated as the bash-compatible ground truth for that case.
Those annotations mean "this is what bash does even though the feature is effectively unimplemented
there", not "skip this because bash does not matter".

#### 1.3 Skip and xfail management

Two-tier approach:
- **File-level skip:** Entire files in the CLI-only, non-applicable, or explicit non-goal lists
- **Case-level xfail:** Individual cases that use unimplemented features

Options for xfail tracking:
1. **TOML sidecar files** — e.g., `tests/fixtures/oils/array.xfail.toml` with case names
2. **Inline annotation** — maintain a `HashMap<&str, Vec<&str>>` in the harness
3. **Convention-based** — xfail everything by default, maintain a pass-list instead

Recommended: **Option 3 (pass-list)**. Start with everything as xfail. Maintain a pass-list per
file. When a case passes that's not on the pass-list, it's an unexpected pass (forces promotion).
This **inverts** the M6.12 xfail model: instead of marking the known failures, you mark the known
passes. That is a better fit here because the imported Oils corpus will initially have many more
expected failures than expected passes.

#### 1.4 Test discovery

Reuse the existing `datatest-stable` pattern from `tests/comparison.rs` and
`tests/spec_tests.rs` so the new suite follows repo conventions and gets one test per input file:

```rust
fn run_oils_spec_file(path: &Path) -> datatest_stable::Result<()> {
    // parse file, apply skip/pass-list rules, run each case, and summarize
    Ok(())
}

datatest_stable::harness! {
    { test = run_oils_spec_file, root = "tests/fixtures/oils", pattern = r".*\.test\.sh$" },
}
```

#### 1.5 `Cargo.toml` wiring

Add a new test target alongside `comparison` and `spec_tests`:

```toml
[[test]]
name = "oils_spec"
harness = false
```

No new discovery crate is required — `datatest-stable` is already in the repo and matches the
existing harness style.

### Phase 2: Import test files

1. Copy all 136 `.test.sh` files from `../just-bash/src/spec-tests/bash/cases/` into
   `tests/fixtures/oils/`
2. Add Apache 2.0 LICENSE notice in `tests/fixtures/oils/LICENSE` (attribute Oils project)
3. Create the initial pass-list by running all cases and recording which pass
4. Document the import provenance clearly in the copied corpus directory and in the relevant docs

### Phase 3: Progressive promotion

As features get implemented in subsequent milestones:
1. Run the Oils suite — unexpected passes appear
2. Add newly passing case names to the pass-list
3. Track pass-rate growth per milestone

### Phase 4: Comparison depth parity (optional)

To close the −263 comparison fixture gap:
- Add more cases to existing `.toml` files (arrays, redirections, builtins)
- Create new fixture files for areas covered by just-bash but not rust-bash
- This is lower priority since the Oils corpus covers the same features more thoroughly

---

## Milestone mapping summary

How importing Oils would improve coverage against the actual guidebook milestones:

| Milestone | Direct benefit from Oils import | Notes |
|---|---|---|
| **M1 — Core Shell** | **Major** | The largest payoff: ~1,200 directly relevant shell-language cases |
| **M2 — Text Processing** | None | Oils does not target `grep`, `sed`, `awk`, `jq`, `diff`, etc. |
| **M3 — Execution Safety** | Indirect only | Useful as regression/fuzz input, but not targeted to limits or network policy |
| **M4 — Filesystem Backends** | Indirect only | Exercises VFS semantics through shell behavior, not backend-specific APIs |
| **M5 — Integration** | Limited | CLI/REPL-oriented Oils files need a separate harness, not the initial `exec()` harness |
| **M6 — Shell Language Completeness** | **Major** | Hundreds of directly relevant cases for arrays, builtins, redirections, options, vars, and process substitution |
| **M7 — Command Coverage & Discoverability** | Minimal | Oils has little direct coverage for `--help`, command inventory, or agent docs |
| **M8 — Embedded Runtimes & Data Formats** | None | Outside the scope of the Oils corpus |
| **M9 — Platform, Security & Execution API** | Indirect only | Imported cases can later feed fuzzing/differential work, but do not directly cover M9 features |

For milestone accounting, the most important point is simple: **Oils materially strengthens M1 and
M6**, and only indirectly helps the rest of the roadmap.

---

## Effort estimate

| Phase | Scope | Size |
|---|---|---|
| Phase 1 (parser + harness) | ~300 lines Rust, new test file | **M** |
| Phase 2 (import + initial pass-list) | File copy, one test run, triage | **S** |
| Phase 3 (progressive promotion) | Ongoing per milestone | **Continuous** |
| Phase 4 (comparison depth) | Optional, ~100 new TOML cases | **S** |

**Total for initial import: 1–2 sessions.**

---

## Acceptance criteria

- [ ] Oils parser correctly handles all format variants (single-line stdout, multiline STDOUT/END,
      multiline STDERR/END, status, bash-specific overrides, stderr-json)
- [ ] All 136 `.test.sh` files are imported under `tests/fixtures/oils/`
- [ ] Apache 2.0 LICENSE attribution present
- [ ] `Cargo.toml` registers the new `oils_spec` test target with `harness = false`
- [ ] File-level skip list excludes CLI-only, non-applicable, and explicit non-goal files
- [ ] Pass-list generated from initial run
- [ ] `cargo test --test oils_spec` runs cleanly (no unexpected failures)
- [ ] Per-file and per-milestone summary printed (reuse M6.12 summary format)
- [ ] Initial run establishes and documents a baseline pass/xfail/skip distribution
- [ ] Unexpected passes force promotion (same unexpected-pass discipline as comparison fixtures)
- [ ] Documentation updated (guidebook chapters 9 and 10)
