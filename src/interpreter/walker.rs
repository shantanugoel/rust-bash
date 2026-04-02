//! AST walking: execution of programs, compound lists, pipelines, and simple commands.

use crate::commands::{CommandContext, CommandResult};
use crate::error::RustBashError;
use crate::interpreter::builtins::{self, resolve_path};
use crate::interpreter::expansion::{expand_word_mut, expand_word_to_string_mut};
use crate::interpreter::{
    CallFrame, ExecResult, ExecutionCounters, FunctionDef, InterpreterState, PersistentFd,
    Variable, VariableAttrs, VariableValue, execute_trap, parse, set_array_element, set_variable,
};

use brush_parser::ast;
use brush_parser::ast::SourceLocation;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// ── xtrace helpers ──────────────────────────────────────────────────

/// Expand PS4 through the normal parameter expansion engine so that
/// variables like `$x` or `$?` inside PS4 are evaluated.
fn expand_ps4(state: &mut InterpreterState) -> String {
    let raw = state
        .env
        .get("PS4")
        .map(|v| v.value.as_scalar().to_string());
    match raw {
        Some(s) if !s.is_empty() => {
            let word = brush_parser::ast::Word {
                value: s,
                loc: Default::default(),
            };
            expand_word_to_string_mut(&word, state).unwrap_or_else(|_| "+ ".to_string())
        }
        Some(_) => "+ ".to_string(), // PS4 is set but empty → default
        None => String::new(),       // PS4 is unset → no prefix
    }
}

/// Quote a single word for xtrace output.  Bash quotes words that contain
/// whitespace, single quotes, double quotes, backslashes, or non-printable
/// characters.  The quoting style uses single quotes with the `'\''` escape
/// for embedded single quotes, except when $'...' is needed for control chars.
fn xtrace_quote(word: &str) -> String {
    if word.is_empty() {
        return "''".to_string();
    }

    // Check what quoting is needed
    let has_single_quote = word.contains('\'');
    let needs_quoting = word
        .chars()
        .any(|c| c.is_whitespace() || c == '\'' || c == '"' || c == '\\' || (c as u32) < 0x20);

    if !needs_quoting {
        return word.to_string();
    }

    // Bash xtrace uses single quotes for most quoting, but represents
    // single quotes as \' (breaking out of quoting).
    // E.g., "it's" → 'it'\''s'
    // But a bare single quote → \'
    if has_single_quote {
        let mut out = String::new();
        let mut in_squote = false;
        for c in word.chars() {
            if c == '\'' {
                if in_squote {
                    out.push('\''); // close single quote
                    in_squote = false;
                }
                out.push_str("\\'");
            } else {
                if !in_squote {
                    out.push('\''); // open single quote
                    in_squote = true;
                }
                out.push(c);
            }
        }
        if in_squote {
            out.push('\'');
        }
        out
    } else {
        // Simple single-quote wrapping (no single quotes in content)
        // Bash puts literal tabs and newlines inside single quotes
        format!("'{word}'")
    }
}

/// Format an xtrace line for a simple command invocation.
fn format_xtrace_command(ps4: &str, cmd: &str, args: &[String]) -> String {
    let mut parts = Vec::with_capacity(1 + args.len());
    parts.push(xtrace_quote(cmd));
    for a in args {
        parts.push(xtrace_quote(a));
    }
    format!("{ps4}{}\n", parts.join(" "))
}

fn current_indexed_array_for_trace(
    name: &str,
    state: &InterpreterState,
) -> std::collections::BTreeMap<usize, String> {
    match state.env.get(name) {
        Some(var) => match &var.value {
            VariableValue::IndexedArray(map) => map.clone(),
            VariableValue::Scalar(s) if s.is_empty() => std::collections::BTreeMap::new(),
            VariableValue::Scalar(s) => {
                let mut map = std::collections::BTreeMap::new();
                map.insert(0, s.clone());
                map
            }
            VariableValue::AssociativeArray(_) => std::collections::BTreeMap::new(),
        },
        None => std::collections::BTreeMap::new(),
    }
}

fn format_xtrace_indexed_assignment(
    name: &str,
    elements: &std::collections::BTreeMap<usize, String>,
    append: bool,
    state: &InterpreterState,
) -> String {
    if !append {
        let vals: Vec<String> = elements.values().map(|v| xtrace_quote(v)).collect();
        return format!("{name}=({})", vals.join(" "));
    }

    let base = current_indexed_array_for_trace(name, state);
    let next_index = base.keys().next_back().map(|k| k + 1).unwrap_or(0);
    let appended: Vec<(usize, &String)> = elements
        .iter()
        .filter_map(|(idx, value)| match base.get(idx) {
            Some(existing) if existing == value => None,
            _ => Some((*idx, value)),
        })
        .collect();

    if appended.is_empty() {
        return format!("{name}+=()");
    }

    let is_plain_append = appended
        .iter()
        .enumerate()
        .all(|(offset, (idx, _))| *idx == next_index + offset);

    if is_plain_append {
        let vals: Vec<String> = appended.iter().map(|(_, v)| xtrace_quote(v)).collect();
        format!("{name}+=({})", vals.join(" "))
    } else {
        let vals: Vec<String> = appended
            .iter()
            .map(|(idx, value)| format!("[{idx}]={}", xtrace_quote(value)))
            .collect();
        format!("{name}+=({})", vals.join(" "))
    }
}

/// Check the errexit (`set -e`) condition after a command completes.
/// If errexit is enabled, the last exit code was non-zero, and we're not
/// in a suppressed context (if/while/until condition, `&&`/`||` left side,
/// `!` pipeline), set `should_exit = true`.
fn check_errexit(state: &mut InterpreterState) {
    if state.shell_opts.errexit
        && state.last_exit_code != 0
        && state.errexit_suppressed == 0
        && !state.in_trap
    {
        state.should_exit = true;
    }
}

/// Check execution limits and return an error if any are exceeded.
fn check_limits(state: &InterpreterState) -> Result<(), RustBashError> {
    if state.counters.command_count > state.limits.max_command_count {
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_command_count",
            limit_value: state.limits.max_command_count,
            actual_value: state.counters.command_count,
        });
    }
    if state.counters.output_size > state.limits.max_output_size {
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_output_size",
            limit_value: state.limits.max_output_size,
            actual_value: state.counters.output_size,
        });
    }
    if state.counters.start_time.elapsed() > state.limits.max_execution_time {
        return Err(RustBashError::Timeout);
    }
    Ok(())
}

/// Execute a parsed program.
pub fn execute_program(
    program: &ast::Program,
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let mut result = ExecResult::default();

    for complete_command in &program.complete_commands {
        if state.should_exit {
            break;
        }
        let r = execute_compound_list(complete_command, state, "")?;
        state.counters.output_size += r.stdout.len() + r.stderr.len();
        check_limits(state)?;
        result.stdout.push_str(&r.stdout);
        result.stderr.push_str(&r.stderr);
        result.exit_code = r.exit_code;
        state.last_exit_code = r.exit_code;
    }

    Ok(result)
}

fn execute_compound_list(
    list: &ast::CompoundList,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let mut result = ExecResult::default();

    for item in &list.0 {
        if state.should_exit || state.control_flow.is_some() {
            break;
        }
        let ast::CompoundListItem(and_or_list, _separator) = item;
        let r = match execute_and_or_list(and_or_list, state, stdin) {
            Ok(r) => r,
            Err(RustBashError::Execution(msg)) if msg.contains("unbound variable") => {
                // nounset errors: print to stderr, exit with code 1
                state.should_exit = true;
                state.last_exit_code = 1;
                ExecResult {
                    stderr: format!("rust-bash: {msg}\n"),
                    exit_code: 1,
                    ..Default::default()
                }
            }
            Err(RustBashError::Execution(msg)) if msg.contains("arithmetic:") => {
                // Arithmetic errors are non-fatal: print to stderr, set $?=1, continue
                state.last_exit_code = 1;
                ExecResult {
                    stderr: format!("rust-bash: {msg}\n"),
                    exit_code: 1,
                    ..Default::default()
                }
            }
            Err(RustBashError::Execution(msg)) if msg.contains("bad substitution") => {
                // Bad substitution errors are non-fatal: print to stderr, set $?=1
                state.last_exit_code = 1;
                ExecResult {
                    stderr: format!("rust-bash: {msg}\n"),
                    exit_code: 1,
                    ..Default::default()
                }
            }
            Err(RustBashError::Execution(msg)) if msg.contains("invalid indirect expansion") => {
                state.last_exit_code = 1;
                ExecResult {
                    stderr: format!("rust-bash: {msg}\n"),
                    exit_code: 1,
                    ..Default::default()
                }
            }
            Err(e) => return Err(e),
        };
        result.stdout.push_str(&r.stdout);
        result.stderr.push_str(&r.stderr);
        result.exit_code = r.exit_code;
        state.last_exit_code = r.exit_code;

        // Fire ERR trap on non-zero exit code (but not when errexit is suppressed,
        // e.g. inside `if`/`while`/`until` conditions or `&&`/`||` chains).
        if r.exit_code != 0
            && !state.in_trap
            && state.errexit_suppressed == 0
            && let Some(err_cmd) = state.traps.get("ERR").cloned()
            && !err_cmd.is_empty()
        {
            let trap_r = execute_trap(&err_cmd, state)?;
            result.stdout.push_str(&trap_r.stdout);
            result.stderr.push_str(&trap_r.stderr);
        }
    }

    Ok(result)
}

fn execute_and_or_list(
    aol: &ast::AndOrList,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    // If there are && or || operators, the first pipeline is on the left side
    // of the chain, so errexit is suppressed for it.
    let has_chain = !aol.additional.is_empty();
    if has_chain {
        state.errexit_suppressed += 1;
    }
    let mut result = execute_pipeline(&aol.first, state, stdin)?;
    if has_chain {
        state.errexit_suppressed -= 1;
    }
    state.last_exit_code = result.exit_code;

    // Check errexit after the first pipeline if it's standalone (no chain)
    if !has_chain {
        check_errexit(state);
        if state.should_exit {
            return Ok(result);
        }
    }

    for (idx, and_or) in aol.additional.iter().enumerate() {
        if state.should_exit || state.control_flow.is_some() {
            break;
        }
        let (should_run, pipeline) = match and_or {
            ast::AndOr::And(p) => (result.exit_code == 0, p),
            ast::AndOr::Or(p) => (result.exit_code != 0, p),
        };
        if should_run {
            // Suppress errexit for all but the last pipeline in the chain
            let is_last = idx == aol.additional.len() - 1;
            if !is_last {
                state.errexit_suppressed += 1;
            }
            let r = execute_pipeline(pipeline, state, stdin)?;
            if !is_last {
                state.errexit_suppressed -= 1;
            }
            result.stdout.push_str(&r.stdout);
            result.stderr.push_str(&r.stderr);
            result.exit_code = r.exit_code;
            state.last_exit_code = r.exit_code;

            if is_last {
                check_errexit(state);
            }
        }
    }

    Ok(result)
}

fn execute_pipeline(
    pipeline: &ast::Pipeline,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let timed = pipeline.timed.is_some();
    let start = if timed {
        Some(crate::platform::Instant::now())
    } else {
        None
    };

    let mut pipe_data = stdin.to_string();
    let mut pipe_data_bytes: Option<Vec<u8>> = None;
    let mut combined_stderr = String::new();
    let mut exit_code = 0;
    let mut exit_codes: Vec<i32> = Vec::new();
    let is_actual_pipe = pipeline.seq.len() > 1;
    let saved_stdin_offset = state.stdin_offset;

    // Negated pipelines (`! cmd`) suppress errexit for the inner commands
    if pipeline.bang {
        state.errexit_suppressed += 1;
    }

    for (idx, command) in pipeline.seq.iter().enumerate() {
        if state.should_exit || state.control_flow.is_some() {
            break;
        }
        // Reset stdin offset when entering a new pipe stage with fresh data
        if idx > 0 {
            state.stdin_offset = 0;
        }
        // Propagate binary data from previous stage via interpreter state
        state.pipe_stdin_bytes = pipe_data_bytes.take();
        let r = execute_command(command, state, &pipe_data)?;
        // If the command produced binary output, use it for next stage
        if let Some(bytes) = r.stdout_bytes {
            pipe_data_bytes = Some(bytes);
            pipe_data = String::new();
        } else {
            pipe_data = r.stdout;
            pipe_data_bytes = None;
        }
        combined_stderr.push_str(&r.stderr);
        exit_code = r.exit_code;
        exit_codes.push(r.exit_code);
    }
    // Clear any leftover binary state
    state.pipe_stdin_bytes = None;

    // Multi-stage pipelines operate on ephemeral pipe data — restore the
    // caller's stdin offset so enclosing loops (e.g. `while read`) are not
    // corrupted by inner pipe stages resetting the offset.
    if is_actual_pipe {
        state.stdin_offset = saved_stdin_offset;
    }

    if pipeline.bang {
        state.errexit_suppressed -= 1;
    }

    // pipefail: exit code = rightmost non-zero, or 0 if all succeeded
    if state.shell_opts.pipefail {
        exit_code = exit_codes
            .iter()
            .rev()
            .copied()
            .find(|&c| c != 0)
            .unwrap_or(0);
    }

    let exit_code = if pipeline.bang {
        i32::from(exit_code == 0)
    } else {
        exit_code
    };

    // Set PIPESTATUS indexed array with each command's exit code.
    // Overwritten on every pipeline (including single commands).
    let mut pipestatus_map = std::collections::BTreeMap::new();
    for (i, code) in exit_codes.iter().enumerate() {
        pipestatus_map.insert(i, code.to_string());
    }
    state.env.insert(
        "PIPESTATUS".to_string(),
        Variable {
            value: VariableValue::IndexedArray(pipestatus_map),
            attrs: VariableAttrs::empty(),
        },
    );

    // Emit timing output for `time` keyword
    if let Some(start) = start {
        let elapsed = start.elapsed();
        let total_secs = elapsed.as_secs_f64();
        let mins = total_secs as u64 / 60;
        let secs = total_secs - (mins as f64 * 60.0);
        combined_stderr.push_str(&format!(
            "\nreal\t{}m{:.3}s\nuser\t0m0.000s\nsys\t0m0.000s\n",
            mins, secs
        ));
    }

    // At the pipeline boundary, convert binary output to lossy string if needed
    let final_stdout = if let Some(bytes) = pipe_data_bytes {
        String::from_utf8_lossy(&bytes).into_owned()
    } else {
        pipe_data
    };

    Ok(ExecResult {
        stdout: final_stdout,
        stderr: combined_stderr,
        exit_code,
        stdout_bytes: None,
    })
}

fn execute_command(
    command: &ast::Command,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    // Update LINENO from the AST node's source position.
    if let Some(loc) = command.location() {
        state.current_lineno = loc.start.line;
    }

    // noexec: skip all commands except simple commands named "set"
    if state.shell_opts.noexec && !matches!(command, ast::Command::Simple(_)) {
        return Ok(ExecResult::default());
    }

    let result = match command {
        ast::Command::Simple(simple_cmd) => execute_simple_command(simple_cmd, state, stdin),
        ast::Command::Compound(compound, redirects) => {
            execute_compound_command(compound, redirects.as_ref(), state, stdin)
        }
        ast::Command::Function(func_def) => {
            match expand_word_to_string_mut(&func_def.fname, state) {
                Ok(name) => {
                    state.functions.insert(
                        name,
                        FunctionDef {
                            body: func_def.body.clone(),
                            source: state.current_source.clone(),
                            source_text: state.current_source_text.clone(),
                            lineno: state.current_lineno,
                        },
                    );
                    Ok(ExecResult::default())
                }
                Err(e) => Err(e),
            }
        }
        ast::Command::ExtendedTest(ext_test, _redirects) => {
            execute_extended_test(&ext_test.expr, state)
        }
    };

    match result {
        Err(RustBashError::ExpansionError {
            message,
            exit_code,
            should_exit,
        }) => {
            state.last_exit_code = exit_code;
            if should_exit {
                state.should_exit = true;
                state.fatal_expansion_error = true;
            }
            Ok(ExecResult {
                stderr: format!("rust-bash: {message}\n"),
                exit_code,
                ..Default::default()
            })
        }
        Err(RustBashError::FailGlob { pattern }) => {
            state.last_exit_code = 1;
            Ok(ExecResult {
                stderr: format!("rust-bash: no match: {pattern}\n"),
                exit_code: 1,
                ..Default::default()
            })
        }
        other => other,
    }
}

// ── Assignment processing ────────────────────────────────────────────

/// A processed assignment ready to be applied to the interpreter state.
#[derive(Debug, Clone)]
enum Assignment {
    /// `name=value` — simple scalar assignment
    Scalar { name: String, value: String },
    /// `name=(val1 val2 ...)` — indexed array assignment
    IndexedArray {
        name: String,
        elements: std::collections::BTreeMap<usize, String>,
        append: bool,
    },
    /// `declare -A name=([k]=v ...)` — associative array assignment
    AssocArray {
        name: String,
        elements: std::collections::BTreeMap<String, String>,
        append: bool,
        warnings: Vec<String>,
    },
    /// `name[index]=value` — single array element
    ArrayElement {
        name: String,
        index: String,
        value: String,
    },
    /// `name[index]+=value` — append to single array element
    AppendArrayElement {
        name: String,
        index: String,
        value: String,
    },
    /// `name+=value` — append to scalar
    AppendScalar { name: String, value: String },
}

impl Assignment {
    fn name(&self) -> &str {
        match self {
            Assignment::Scalar { name, .. }
            | Assignment::IndexedArray { name, .. }
            | Assignment::AssocArray { name, .. }
            | Assignment::ArrayElement { name, .. }
            | Assignment::AppendArrayElement { name, .. }
            | Assignment::AppendScalar { name, .. } => name,
        }
    }
}

/// Process an AST assignment into our internal Assignment type.
fn process_assignment(
    assignment: &ast::Assignment,
    append: bool,
    state: &mut InterpreterState,
) -> Result<Assignment, RustBashError> {
    match (&assignment.name, &assignment.value) {
        (ast::AssignmentName::VariableName(name), ast::AssignmentValue::Scalar(w)) => {
            let value = expand_word_to_string_mut(w, state)?;
            if append {
                Ok(Assignment::AppendScalar {
                    name: name.clone(),
                    value,
                })
            } else {
                Ok(Assignment::Scalar {
                    name: name.clone(),
                    value,
                })
            }
        }
        (ast::AssignmentName::VariableName(name), ast::AssignmentValue::Array(items)) => {
            // Check if target is an associative array
            let is_assoc = state
                .env
                .get(name)
                .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)));
            if is_assoc {
                let (elements, warnings) =
                    process_assoc_array_initializer(name, items, append, state)?;
                Ok(Assignment::AssocArray {
                    name: name.clone(),
                    elements,
                    append,
                    warnings,
                })
            } else {
                let elements = process_indexed_array_initializer(name, items, append, state)?;
                Ok(Assignment::IndexedArray {
                    name: name.clone(),
                    elements,
                    append,
                })
            }
        }
        (
            ast::AssignmentName::ArrayElementName(name, index_str),
            ast::AssignmentValue::Scalar(w),
        ) => {
            let value = expand_word_to_string_mut(w, state)?;
            // Expand index — it may contain variable references
            let index_word = ast::Word {
                value: index_str.clone(),
                loc: None,
            };
            let expanded_index = expand_word_to_string_mut(&index_word, state)?;
            if append {
                Ok(Assignment::AppendArrayElement {
                    name: name.clone(),
                    index: expanded_index,
                    value,
                })
            } else {
                Ok(Assignment::ArrayElement {
                    name: name.clone(),
                    index: expanded_index,
                    value,
                })
            }
        }
        (ast::AssignmentName::ArrayElementName(name, _), ast::AssignmentValue::Array(_)) => Err(
            RustBashError::Execution(format!("{name}: cannot assign array to array element")),
        ),
    }
}

#[derive(Debug, Clone)]
enum IndexedArrayOp {
    Bare(Vec<String>),
    Set { index_expr: String, value: String },
    Append { index_expr: String, value: String },
}

#[derive(Debug, Clone)]
enum AssocArrayOp {
    Bare(Vec<String>),
    Set { key: String, value: String },
    Append { key: String, value: String },
}

#[derive(Debug, Clone)]
struct ParsedArrayToken {
    index_expr: String,
    value_expr: String,
    append: bool,
}

fn reconstruct_array_item_raw(opt_idx_word: &Option<ast::Word>, val_word: &ast::Word) -> String {
    if let Some(idx_word) = opt_idx_word {
        format!("[{}]={}", idx_word.value, val_word.value)
    } else {
        val_word.value.clone()
    }
}

fn parse_array_item_token(raw: &str) -> Option<ParsedArrayToken> {
    if !raw.starts_with('[') {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut depth = 1usize;
    let mut i = 1usize;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    let rest = &raw[i + 1..];
                    if let Some(value_expr) = rest.strip_prefix("+=") {
                        return Some(ParsedArrayToken {
                            index_expr: raw[1..i].to_string(),
                            value_expr: value_expr.to_string(),
                            append: true,
                        });
                    }
                    if let Some(value_expr) = rest.strip_prefix('=') {
                        return Some(ParsedArrayToken {
                            index_expr: raw[1..i].to_string(),
                            value_expr: value_expr.to_string(),
                            append: false,
                        });
                    }
                    return None;
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn expand_raw_array_word(
    raw: &str,
    state: &mut InterpreterState,
) -> Result<Vec<String>, RustBashError> {
    let word = ast::Word {
        value: raw.to_string(),
        loc: None,
    };
    expand_word_mut(&word, state)
}

fn expand_raw_array_word_to_string(
    raw: &str,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    let word = ast::Word {
        value: raw.to_string(),
        loc: None,
    };
    expand_word_to_string_mut(&word, state)
}

fn escape_tildes(raw: &str) -> String {
    raw.replace('~', "\\~")
}

fn normalize_assoc_literal_key(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.len() >= 2
        && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || (trimmed.starts_with('\'') && trimmed.ends_with('\'')))
    {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

fn indexed_array_base_state(
    name: &str,
    append: bool,
    state: &InterpreterState,
) -> (std::collections::BTreeMap<usize, String>, usize) {
    if !append {
        return (std::collections::BTreeMap::new(), 0);
    }

    let map = match state.env.get(name) {
        Some(var) => match &var.value {
            VariableValue::IndexedArray(existing) => existing.clone(),
            VariableValue::Scalar(s) if s.is_empty() => std::collections::BTreeMap::new(),
            VariableValue::Scalar(s) => {
                let mut map = std::collections::BTreeMap::new();
                map.insert(0, s.clone());
                map
            }
            VariableValue::AssociativeArray(_) => std::collections::BTreeMap::new(),
        },
        None => std::collections::BTreeMap::new(),
    };
    let next_index = map.keys().next_back().map(|k| k + 1).unwrap_or(0);
    (map, next_index)
}

fn sync_temp_indexed_array(
    name: &str,
    map: &std::collections::BTreeMap<usize, String>,
    attrs: VariableAttrs,
    state: &mut InterpreterState,
) {
    state.env.insert(
        name.to_string(),
        Variable {
            value: VariableValue::IndexedArray(map.clone()),
            attrs,
        },
    );
}

fn evaluate_indexed_array_index(
    name: &str,
    index_expr: &str,
    state: &mut InterpreterState,
) -> Result<usize, RustBashError> {
    let expanded_index = expand_raw_array_word_to_string(index_expr, state)?;
    let idx = crate::interpreter::arithmetic::eval_arithmetic(&expanded_index, state)?;
    resolve_negative_array_index(idx, name, state)
}

fn process_indexed_array_initializer(
    name: &str,
    items: &[(Option<ast::Word>, ast::Word)],
    append: bool,
    state: &mut InterpreterState,
) -> Result<std::collections::BTreeMap<usize, String>, RustBashError> {
    let mut ops = Vec::new();
    for (opt_idx_word, val_word) in items {
        let raw = reconstruct_array_item_raw(opt_idx_word, val_word);
        let expanded_tokens =
            crate::interpreter::brace::brace_expand(&raw, state.limits.max_brace_expansion)?;
        if expanded_tokens.len() > 1 {
            for token in expanded_tokens {
                let vals = expand_raw_array_word(&token, state)?;
                if vals.is_empty() {
                    ops.push(IndexedArrayOp::Bare(vec![String::new()]));
                } else {
                    ops.push(IndexedArrayOp::Bare(vals));
                }
            }
            continue;
        }

        let token = expanded_tokens
            .into_iter()
            .next()
            .unwrap_or_else(|| raw.clone());
        if let Some(parsed) = parse_array_item_token(&token) {
            let value = expand_raw_array_word_to_string(&parsed.value_expr, state)?;
            if parsed.append {
                ops.push(IndexedArrayOp::Append {
                    index_expr: parsed.index_expr,
                    value,
                });
            } else {
                ops.push(IndexedArrayOp::Set {
                    index_expr: parsed.index_expr,
                    value,
                });
            }
        } else {
            let vals = expand_raw_array_word(&token, state)?;
            if vals.is_empty() {
                ops.push(IndexedArrayOp::Bare(vec![String::new()]));
            } else {
                ops.push(IndexedArrayOp::Bare(vals));
            }
        }
    }

    let saved_var = state.env.get(name).cloned();
    let attrs = saved_var
        .as_ref()
        .map(|var| var.attrs)
        .unwrap_or(VariableAttrs::empty());
    let (mut map, mut auto_idx) = indexed_array_base_state(name, append, state);
    sync_temp_indexed_array(name, &map, attrs, state);

    for op in ops {
        match op {
            IndexedArrayOp::Bare(values) => {
                for value in values {
                    map.insert(auto_idx, value);
                    auto_idx += 1;
                    sync_temp_indexed_array(name, &map, attrs, state);
                }
            }
            IndexedArrayOp::Set { index_expr, value } => {
                let idx = evaluate_indexed_array_index(name, &index_expr, state)?;
                map.insert(idx, value);
                auto_idx = idx + 1;
                sync_temp_indexed_array(name, &map, attrs, state);
            }
            IndexedArrayOp::Append { index_expr, value } => {
                let idx = evaluate_indexed_array_index(name, &index_expr, state)?;
                let current = map.get(&idx).cloned().unwrap_or_default();
                map.insert(idx, format!("{current}{value}"));
                auto_idx = idx + 1;
                sync_temp_indexed_array(name, &map, attrs, state);
            }
        }
    }

    if let Some(var) = saved_var {
        state.env.insert(name.to_string(), var);
    } else {
        state.env.remove(name);
    }

    Ok(map)
}

fn current_assoc_array(
    name: &str,
    state: &InterpreterState,
) -> std::collections::BTreeMap<String, String> {
    match state.env.get(name) {
        Some(var) => match &var.value {
            VariableValue::AssociativeArray(map) => map.clone(),
            _ => std::collections::BTreeMap::new(),
        },
        None => std::collections::BTreeMap::new(),
    }
}

fn assoc_array_warning(name: &str, item: &str, state: &InterpreterState) -> String {
    format!(
        "bash: line {}: {name}: {item}: must use subscript when assigning associative array\n",
        state.current_lineno
    )
}

fn process_assoc_array_initializer(
    name: &str,
    items: &[(Option<ast::Word>, ast::Word)],
    append: bool,
    state: &mut InterpreterState,
) -> Result<(std::collections::BTreeMap<String, String>, Vec<String>), RustBashError> {
    let mut ops = Vec::new();
    for (opt_idx_word, val_word) in items {
        let raw = reconstruct_array_item_raw(opt_idx_word, val_word);
        if let Some(parsed) = parse_array_item_token(&raw) {
            let key = normalize_assoc_literal_key(&parsed.index_expr);
            let value = expand_raw_array_word_to_string(&escape_tildes(&parsed.value_expr), state)?;
            if parsed.append {
                ops.push(AssocArrayOp::Append { key, value });
            } else {
                ops.push(AssocArrayOp::Set { key, value });
            }
            continue;
        }

        let expanded_tokens =
            crate::interpreter::brace::brace_expand(&raw, state.limits.max_brace_expansion)?;
        for token in expanded_tokens {
            let vals = expand_raw_array_word(&token, state)?;
            if vals.is_empty() {
                ops.push(AssocArrayOp::Bare(vec![String::new()]));
            } else {
                ops.push(AssocArrayOp::Bare(vals));
            }
        }
    }

    let mut saw_keyed = false;
    let mut bare_words = Vec::new();
    for op in &ops {
        match op {
            AssocArrayOp::Bare(words) => bare_words.extend(words.iter().cloned()),
            AssocArrayOp::Set { .. } | AssocArrayOp::Append { .. } => saw_keyed = true,
        }
    }

    let existing = current_assoc_array(name, state);
    let mut map = if append {
        existing.clone()
    } else {
        std::collections::BTreeMap::new()
    };
    let mut warnings = Vec::new();

    if !saw_keyed {
        let mut iter = bare_words.into_iter();
        while let Some(key) = iter.next() {
            let value = iter.next().unwrap_or_default();
            map.insert(key, value);
        }
        return Ok((map, warnings));
    }

    for op in ops {
        match op {
            AssocArrayOp::Bare(words) => {
                for word in words {
                    warnings.push(assoc_array_warning(name, &word, state));
                }
            }
            AssocArrayOp::Set { key, value } => {
                map.insert(key, value);
            }
            AssocArrayOp::Append { key, value } => {
                let current = if append {
                    map.get(&key).cloned().unwrap_or_default()
                } else {
                    existing.get(&key).cloned().unwrap_or_default()
                };
                map.insert(key, format!("{current}{value}"));
            }
        }
    }

    Ok((map, warnings))
}

/// Apply a processed assignment to the interpreter state.
fn apply_assignment(
    assignment: Assignment,
    state: &mut InterpreterState,
) -> Result<(), RustBashError> {
    match assignment {
        Assignment::Scalar { name, value } => {
            set_variable(state, &name, value)?;
        }
        Assignment::IndexedArray { name, elements, .. } => {
            if let Some(var) = state.env.get(&name)
                && var.readonly()
            {
                return Err(RustBashError::Execution(format!(
                    "{name}: readonly variable"
                )));
            }
            if elements.len() > state.limits.max_array_elements {
                return Err(RustBashError::LimitExceeded {
                    limit_name: "max_array_elements",
                    limit_value: state.limits.max_array_elements,
                    actual_value: elements.len(),
                });
            }
            let attrs = state
                .env
                .get(&name)
                .map(|v| v.attrs)
                .unwrap_or(VariableAttrs::empty());
            state.env.insert(
                name,
                Variable {
                    value: VariableValue::IndexedArray(elements),
                    attrs,
                },
            );
        }
        Assignment::AssocArray { name, elements, .. } => {
            if let Some(var) = state.env.get(&name)
                && var.readonly()
            {
                return Err(RustBashError::Execution(format!(
                    "{name}: readonly variable"
                )));
            }
            if elements.len() > state.limits.max_array_elements {
                return Err(RustBashError::LimitExceeded {
                    limit_name: "max_array_elements",
                    limit_value: state.limits.max_array_elements,
                    actual_value: elements.len(),
                });
            }
            let attrs = state
                .env
                .get(&name)
                .map(|v| v.attrs)
                .unwrap_or(VariableAttrs::empty());
            state.env.insert(
                name,
                Variable {
                    value: VariableValue::AssociativeArray(elements),
                    attrs,
                },
            );
        }
        Assignment::ArrayElement { name, index, value } => {
            // Check if target is an associative array
            let is_assoc = state
                .env
                .get(&name)
                .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)));
            if is_assoc {
                crate::interpreter::set_assoc_element(state, &name, index, value)?;
            } else {
                // Evaluate index as arithmetic expression
                let idx = crate::interpreter::arithmetic::eval_arithmetic(&index, state)?;
                let uidx = resolve_negative_array_index(idx, &name, state)?;
                set_array_element(state, &name, uidx, value)?;
            }
        }
        Assignment::AppendArrayElement { name, index, value } => {
            let is_assoc = state
                .env
                .get(&name)
                .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)));
            if is_assoc {
                let current = state
                    .env
                    .get(&name)
                    .and_then(|v| match &v.value {
                        VariableValue::AssociativeArray(map) => map.get(&index).cloned(),
                        _ => None,
                    })
                    .unwrap_or_default();
                let new_val = format!("{current}{value}");
                crate::interpreter::set_assoc_element(state, &name, index, new_val)?;
            } else {
                let idx = crate::interpreter::arithmetic::eval_arithmetic(&index, state)?;
                let uidx = resolve_negative_array_index(idx, &name, state)?;
                let current = state
                    .env
                    .get(&name)
                    .and_then(|v| match &v.value {
                        VariableValue::IndexedArray(map) => map.get(&uidx).cloned(),
                        VariableValue::Scalar(s) if uidx == 0 => Some(s.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                let new_val = format!("{current}{value}");
                set_array_element(state, &name, uidx, new_val)?;
            }
        }
        Assignment::AppendScalar { name, value } => {
            // For integer variables, += performs arithmetic addition.
            let target = crate::interpreter::resolve_nameref(&name, state)?;
            let is_integer = state
                .env
                .get(&target)
                .is_some_and(|v| v.attrs.contains(VariableAttrs::INTEGER));
            if is_integer {
                let current = state
                    .env
                    .get(&target)
                    .map(|v| v.value.as_scalar().to_string())
                    .unwrap_or_else(|| "0".to_string());
                let expr = format!("{current}+{value}");
                set_variable(state, &name, expr)?;
            } else {
                match state.env.get(&target) {
                    Some(var) => {
                        let new_val = format!("{}{}", var.value.as_scalar(), value);
                        set_variable(state, &name, new_val)?;
                    }
                    None => {
                        set_variable(state, &name, value)?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Resolve a negative array index to a positive one based on the current max key.
/// In bash, `a[-1]` refers to the last element (max_key), `a[-2]` to the one before, etc.
fn resolve_negative_array_index(
    idx: i64,
    name: &str,
    state: &InterpreterState,
) -> Result<usize, RustBashError> {
    if idx >= 0 {
        return Ok(idx as usize);
    }
    let max_key = state.env.get(name).and_then(|v| match &v.value {
        VariableValue::IndexedArray(map) => map.keys().next_back().copied(),
        VariableValue::Scalar(_) => Some(0),
        _ => None,
    });
    match max_key {
        Some(mk) => {
            let resolved = mk as i64 + 1 + idx;
            if resolved < 0 {
                Err(RustBashError::Execution(format!(
                    "{name}: bad array subscript"
                )))
            } else {
                Ok(resolved as usize)
            }
        }
        None => Err(RustBashError::Execution(format!(
            "{name}: bad array subscript"
        ))),
    }
}

/// Apply an assignment, converting `Execution` errors to shell errors (stderr +
/// exit code 1) instead of propagating them as fatal `Err`. This matches bash
/// behavior where assignment errors (e.g. nameref cycles, readonly) in bare
/// assignment context print an error and set `$?` but do not abort the script.
fn apply_assignment_shell_error(
    assignment: Assignment,
    state: &mut InterpreterState,
    result: &mut ExecResult,
) -> Result<(), RustBashError> {
    if let Assignment::AssocArray { warnings, .. } = &assignment {
        for warning in warnings {
            result.stderr.push_str(warning);
        }
    }
    match apply_assignment(assignment, state) {
        Ok(()) => Ok(()),
        Err(RustBashError::Execution(msg)) => {
            result.stderr.push_str(&format!("rust-bash: {msg}\n"));
            result.exit_code = 1;
            state.last_exit_code = 1;
            Ok(())
        }
        Err(other) => Err(other),
    }
}

fn suffix_assignment_words_are_special(cmd_word: &ast::Word, cmd_name: &str) -> bool {
    if cmd_word.value.contains('$') || cmd_word.value.contains('`') {
        return false;
    }

    matches!(
        cmd_name,
        "declare" | "export" | "local" | "readonly" | "typeset"
    )
}

fn special_assignment_is_assoc_literal(cmd_name: &str, args: &[String]) -> bool {
    matches!(cmd_name, "declare" | "local" | "readonly" | "typeset")
        && args.iter().any(|arg| {
            arg.strip_prefix('-')
                .is_some_and(|flags| flags.chars().any(|flag| flag == 'A'))
        })
}

fn word_is_simply_quoted_literal(word: &ast::Word) -> bool {
    let value = &word.value;
    (value.starts_with('\'') && value.ends_with('\'') && value.len() >= 2)
        || (value.starts_with('"') && value.ends_with('"') && value.len() >= 2)
}

fn expand_alias_command(cmd_name: &str, state: &InterpreterState) -> (String, Vec<String>) {
    if !state.shopt_opts.expand_aliases {
        return (cmd_name.to_string(), Vec::new());
    }

    let Some(expansion) = state.aliases.get(cmd_name) else {
        return (cmd_name.to_string(), Vec::new());
    };

    let mut parts: Vec<String> = expansion
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if parts.is_empty() {
        return (cmd_name.to_string(), Vec::new());
    }

    let new_cmd = parts.remove(0);
    (new_cmd, parts)
}

fn execute_simple_command(
    cmd: &ast::SimpleCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    // noexec: skip all simple commands (bash behavior: once set -n is active, nothing runs)
    if state.shell_opts.noexec {
        return Ok(ExecResult::default());
    }

    // 1. Collect redirections and assignments from prefix
    let mut assignments: Vec<Assignment> = Vec::new();
    let mut redirects: Vec<&ast::IoRedirect> = Vec::new();
    // Track process substitution temp files for cleanup
    let mut proc_sub_temps: Vec<String> = Vec::new();
    // Track deferred write process substitutions: (inner command list, temp path)
    let mut deferred_write_subs: Vec<(&ast::CompoundList, String)> = Vec::new();
    // Keep raw AST assignments for deferred expansion in temp-binding path
    let mut raw_assignments: Vec<(&ast::Assignment, bool)> = Vec::new();

    // Save exit code before prefix expansion to detect command sub execution
    let exit_before_prefix = state.last_exit_code;

    if let Some(prefix) = &cmd.prefix {
        for item in &prefix.0 {
            match item {
                ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, _word) => {
                    raw_assignments.push((assignment, assignment.append));
                }
                ast::CommandPrefixOrSuffixItem::IoRedirect(redir) => {
                    redirects.push(redir);
                }
                ast::CommandPrefixOrSuffixItem::ProcessSubstitution(kind, subshell) => {
                    let path = expand_process_substitution(
                        kind,
                        &subshell.list,
                        state,
                        &mut deferred_write_subs,
                    )?;
                    proc_sub_temps.push(path);
                }
                _ => {}
            }
        }
    }

    // 2. Expand command name
    let raw_cmd_name = cmd
        .word_or_name
        .as_ref()
        .map(|w| expand_word_to_string_mut(w, state))
        .transpose()?;
    let alias_expansion = raw_cmd_name
        .as_ref()
        .map(|name| expand_alias_command(name, state));
    let special_suffix_assignments = cmd
        .word_or_name
        .as_ref()
        .zip(alias_expansion.as_ref())
        .is_some_and(|(word, (expanded, _))| suffix_assignment_words_are_special(word, expanded));

    // 3. Expand arguments and collect redirections from suffix
    let mut args: Vec<String> = alias_expansion
        .as_ref()
        .map(|(_, alias_args)| alias_args.clone())
        .unwrap_or_default();
    if let Some(suffix) = &cmd.suffix {
        for item in &suffix.0 {
            match item {
                ast::CommandPrefixOrSuffixItem::Word(w) => match expand_word_mut(w, state) {
                    Ok(expanded) => args.extend(expanded),
                    Err(RustBashError::FailGlob { pattern }) => {
                        state.last_exit_code = 1;
                        return Ok(ExecResult {
                            stderr: format!("rust-bash: no match: {pattern}\n"),
                            exit_code: 1,
                            ..Default::default()
                        });
                    }
                    Err(e) => return Err(e),
                },
                ast::CommandPrefixOrSuffixItem::IoRedirect(redir) => {
                    redirects.push(redir);
                }
                ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, word) => {
                    if !special_suffix_assignments {
                        args.extend(expand_word_mut(word, state)?);
                        continue;
                    }
                    let name = match &assignment.name {
                        ast::AssignmentName::VariableName(n) => n.clone(),
                        ast::AssignmentName::ArrayElementName(n, idx) => {
                            let idx_word = ast::Word {
                                value: idx.clone(),
                                loc: None,
                            };
                            let expanded_idx = expand_word_to_string_mut(&idx_word, state)?;
                            format!("{n}[{expanded_idx}]")
                        }
                    };
                    match &assignment.value {
                        ast::AssignmentValue::Scalar(w) => {
                            let value = expand_word_to_string_mut(w, state)?;
                            args.push(format!("{name}={value}"));
                        }
                        ast::AssignmentValue::Array(items) => {
                            let assoc_literal = alias_expansion
                                .as_ref()
                                .map(|(name, _)| special_assignment_is_assoc_literal(name, &args))
                                .unwrap_or(false);
                            let assign_op = if assignment.append { "+=" } else { "=" };
                            let mut parts = Vec::new();
                            for (opt_idx_word, val_word) in items {
                                if assoc_literal
                                    && let Some(idx_word) = opt_idx_word
                                    && val_word.value.contains('~')
                                {
                                    parts.push(format!("[{}]={}", idx_word.value, val_word.value));
                                    continue;
                                }
                                let vals = expand_word_mut(val_word, state)?;
                                if let Some(idx_word) = opt_idx_word {
                                    let idx_str = if assoc_literal
                                        && word_is_simply_quoted_literal(idx_word)
                                    {
                                        idx_word.value.clone()
                                    } else {
                                        expand_word_to_string_mut(idx_word, state)?
                                    };
                                    let first = vals.first().cloned().unwrap_or_default();
                                    if first.is_empty() {
                                        parts.push(format!("[{idx_str}]=''"));
                                    } else {
                                        parts.push(format!("[{idx_str}]={first}"));
                                    }
                                    for v in vals.into_iter().skip(1) {
                                        if v.is_empty() {
                                            parts.push("''".to_string());
                                        } else {
                                            parts.push(v);
                                        }
                                    }
                                } else {
                                    for v in vals {
                                        if v.is_empty() {
                                            parts.push("''".to_string());
                                        } else {
                                            parts.push(v);
                                        }
                                    }
                                }
                            }
                            args.push(format!("{name}{assign_op}({})", parts.join(" ")));
                        }
                    }
                }
                ast::CommandPrefixOrSuffixItem::ProcessSubstitution(kind, subshell) => {
                    let path = expand_process_substitution(
                        kind,
                        &subshell.list,
                        state,
                        &mut deferred_write_subs,
                    )?;
                    proc_sub_temps.push(path.clone());
                    args.push(path);
                }
            }
        }
    }

    // 4. No command name → persist assignments in environment
    let Some(cmd_name) = alias_expansion.as_ref().map(|(name, _)| name.clone()) else {
        // Expand assignments lazily (no command to run, so no deferred expansion needed)
        for (raw_assign, append) in &raw_assignments {
            assignments.push(process_assignment(raw_assign, *append, state)?);
        }
        // xtrace for bare assignments (e.g. `X=1`)
        if state.shell_opts.xtrace && !assignments.is_empty() {
            let ps4 = expand_ps4(state);
            let mut trace = String::new();
            for a in &assignments {
                let part = match a {
                    Assignment::Scalar { name, value } => format!("{name}={value}"),
                    Assignment::IndexedArray {
                        name,
                        elements,
                        append,
                    } => format_xtrace_indexed_assignment(name, elements, *append, state),
                    Assignment::ArrayElement {
                        name, index, value, ..
                    } => format!("{name}[{index}]={value}"),
                    Assignment::AppendArrayElement {
                        name, index, value, ..
                    } => format!("{name}[{index}]+={value}"),
                    Assignment::AssocArray { name, append, .. } => {
                        let op = if *append { "+=" } else { "=" };
                        format!("{name}{op}(...)")
                    }
                    Assignment::AppendScalar { name, value } => format!("{name}+={value}"),
                };
                trace.push_str(&format!("{ps4}{part}\n"));
            }
            // Bare assignments produce no output, but emit xtrace to stderr
            let mut result = ExecResult {
                stderr: trace,
                ..ExecResult::default()
            };
            for a in assignments {
                apply_assignment_shell_error(a, state, &mut result)?;
                if result.exit_code != 0 {
                    break;
                }
            }
            if !state.pending_cmdsub_stderr.is_empty() {
                let cmdsub_stderr = std::mem::take(&mut state.pending_cmdsub_stderr);
                result.stderr = format!("{cmdsub_stderr}{}", result.stderr);
            }
            // Preserve command sub exit code if one ran during expansion;
            // otherwise bare assignments reset $? to 0 (like bash).
            if result.exit_code == 0 && state.last_exit_code != exit_before_prefix {
                result.exit_code = state.last_exit_code;
            }
            return Ok(result);
        }
        let mut result = ExecResult::default();
        for a in assignments {
            apply_assignment_shell_error(a, state, &mut result)?;
            if result.exit_code != 0 {
                break;
            }
        }
        if !state.pending_cmdsub_stderr.is_empty() {
            let cmdsub_stderr = std::mem::take(&mut state.pending_cmdsub_stderr);
            result.stderr = format!("{cmdsub_stderr}{}", result.stderr);
        }
        if result.exit_code == 0 && state.last_exit_code != exit_before_prefix {
            result.exit_code = state.last_exit_code;
        }
        return Ok(result);
    };

    // 4b. Empty command name (e.g. from `$(false)`) → no command, persist assignments
    if cmd_name.is_empty() && args.is_empty() {
        // Expand assignments lazily
        let mut expanded = Vec::new();
        for (raw_assign, append) in &raw_assignments {
            expanded.push(process_assignment(raw_assign, *append, state)?);
        }
        let mut result = ExecResult {
            exit_code: state.last_exit_code,
            ..ExecResult::default()
        };
        for a in expanded {
            apply_assignment_shell_error(a, state, &mut result)?;
            if result.exit_code != 0 {
                break;
            }
        }
        if !state.pending_cmdsub_stderr.is_empty() {
            let cmdsub_stderr = std::mem::take(&mut state.pending_cmdsub_stderr);
            result.stderr = format!("{cmdsub_stderr}{}", result.stderr);
        }
        return Ok(result);
    }

    // 4c. Handle `exec` builtin specially — it needs access to redirects.
    //     Prefix assignments before `exec` are permanent (no subshell).
    if cmd_name == "exec" {
        // Intercept --help before exec dispatch
        if args.first().map(|a| a.as_str()) == Some("--help")
            && let Some(meta) = builtins::builtin_meta("exec")
            && meta.supports_help_flag
        {
            return Ok(ExecResult {
                stdout: crate::commands::format_help(meta),
                stderr: String::new(),
                exit_code: 0,
                stdout_bytes: None,
            });
        }
        for (raw_assign, append) in &raw_assignments {
            let a = process_assignment(raw_assign, *append, state)?;
            let mut dummy = ExecResult::default();
            apply_assignment_shell_error(a, state, &mut dummy)?;
            if dummy.exit_code != 0 {
                return Ok(dummy);
            }
        }
        return execute_exec_builtin(&args, &redirects, state, stdin);
    }

    // 5. Apply temporary pre-command assignments
    // On error (e.g. readonly): print the error but still execute the command.
    // Bash skips the failing assignment but runs the command; $? is from the
    // command, not the assignment error.
    // Re-expand each assignment in order so preceding bindings are visible
    // (e.g. FOO=foo BAR="$FOO" cmd → BAR sees FOO=foo).
    // Array element assignments (e.g. b[0]=2) are silently ignored in env
    // binding context — bash does not export them.
    let mut saved: Vec<(String, Option<Variable>)> = Vec::new();
    let mut temp_binding_scope: HashMap<String, Option<Variable>> = HashMap::new();
    let mut prefix_stderr = String::new();
    for (raw_assign, append) in &raw_assignments {
        let a = match process_assignment(raw_assign, *append, state) {
            Ok(a) => a,
            Err(e) => {
                prefix_stderr.push_str(&format!("rust-bash: {e}\n"));
                continue;
            }
        };
        // Skip array assignments in env binding context (bash ignores them)
        if matches!(
            a,
            Assignment::ArrayElement { .. }
                | Assignment::AppendArrayElement { .. }
                | Assignment::IndexedArray { .. }
                | Assignment::AssocArray { .. }
        ) {
            continue;
        }
        let previous = state.env.get(a.name()).cloned();
        saved.push((a.name().to_string(), previous.clone()));
        temp_binding_scope
            .entry(a.name().to_string())
            .or_insert(previous);
        let mut dummy = ExecResult::default();
        apply_assignment_shell_error(a.clone(), state, &mut dummy)?;
        if dummy.exit_code != 0 {
            prefix_stderr.push_str(&dummy.stderr);
        }
        // Temporary per-command bindings are exported for the duration of
        // the command (bash behavior: `FOO=bar cmd` exports FOO to cmd).
        if let Some(var) = state.env.get_mut(a.name()) {
            var.attrs.insert(VariableAttrs::EXPORTED);
        }
    }
    let has_temp_binding_scope = !temp_binding_scope.is_empty();
    if has_temp_binding_scope {
        state.temp_binding_scopes.push(temp_binding_scope);
    }

    // 5b–8c are wrapped in an immediately-invoked closure so cleanup (8d) and
    // assignment restore (9) run on every exit path, including early `?` returns.
    struct RedirProcSub<'a> {
        temp_path: String,
        kind: &'a ast::ProcessSubstitutionKind,
        list: &'a ast::CompoundList,
    }
    let last_arg = args.last().cloned().unwrap_or_else(|| cmd_name.clone());
    let should_trace = state.shell_opts.xtrace;
    // Expand PS4 BEFORE dispatch so that `local PS4=...` is traced with the old value
    let pre_ps4 = if should_trace {
        Some(expand_ps4(state))
    } else {
        None
    };
    let expansion_stderr = std::mem::take(&mut state.pending_cmdsub_stderr);

    let mut inner_result = (|| -> Result<ExecResult, RustBashError> {
        // 5b. Pre-allocate ALL redirect process substitution temp files in one pass.
        //     This ensures allocation order matches redirect-list order, avoiding
        //     counter mismatch when Read and Write proc-subs are mixed in the same
        //     redirect list (stdin Reads are processed before output Writes).
        let mut redir_proc_subs: Vec<RedirProcSub<'_>> = Vec::new();
        for redir in &redirects {
            if let ast::IoRedirect::File(
                _,
                _,
                target @ ast::IoFileRedirectTarget::ProcessSubstitution(kind, subshell),
            ) = redir
            {
                let temp_path = match kind {
                    ast::ProcessSubstitutionKind::Read => {
                        execute_read_process_substitution(&subshell.list, state)?
                    }
                    ast::ProcessSubstitutionKind::Write => allocate_proc_sub_temp_file(state, b"")?,
                };
                proc_sub_temps.push(temp_path.clone());
                // Key by AST-node address so redirect_target_filename resolves the
                // correct path regardless of the order redirects are visited.
                let key = std::ptr::from_ref(target) as usize;
                state.proc_sub_prealloc.insert(key, temp_path.clone());
                redir_proc_subs.push(RedirProcSub {
                    temp_path,
                    kind,
                    list: &subshell.list,
                });
            }
        }

        // 6. Handle stdin redirection
        let input_redirects = match collect_input_redirects(&redirects, state, stdin) {
            Ok(map) => map,
            Err(RustBashError::RedirectFailed(msg)) => {
                let mut result = ExecResult {
                    stderr: format!("rust-bash: {msg}\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                };
                state.last_exit_code = 1;
                apply_output_redirects(&redirects, &mut result, state)?;
                return Ok(result);
            }
            Err(e) => return Err(e),
        };
        let effective_stdin = match input_redirects.get(&0) {
            Some(s) => s.clone(),
            None => stdin.to_string(),
        };
        let saved_stdin_offset = state.stdin_offset;
        if input_redirects.contains_key(&0) {
            state.stdin_offset = 0;
        }

        // Track last argument for $_ (last argument of the simple command).
        state.last_argument = last_arg.clone();

        // 7a. Capture xtrace state before dispatch (so `set +x` is still traced)
        //     (captured outside closure as `should_trace`)

        // 7. Dispatch command
        let dispatch_result = dispatch_command(
            &cmd_name,
            &args,
            state,
            &effective_stdin,
            Some(&input_redirects),
        );
        if input_redirects.contains_key(&0) {
            state.stdin_offset = saved_stdin_offset;
        }
        let mut result = dispatch_result?;

        // 7a. Emit xtrace to stderr
        if let Some(ref ps4) = pre_ps4 {
            let mut trace = format_xtrace_command(ps4, &cmd_name, &args);
            // Assignment builtins (readonly, declare, export, typeset)
            // also trace each assignment separately.
            // Note: local does NOT trace assignments separately.
            if matches!(
                cmd_name.as_str(),
                "readonly" | "declare" | "typeset" | "export"
            ) {
                for arg in &args {
                    if let Some(eq_pos) = arg.find('=') {
                        let name_part = &arg[..eq_pos];
                        // Only trace if it looks like a variable assignment (not a flag)
                        if !name_part.is_empty()
                            && !name_part.starts_with('-')
                            && name_part
                                .chars()
                                .all(|c| c.is_alphanumeric() || c == '_' || c == '+')
                        {
                            trace.push_str(&format!("{ps4}{arg}\n"));
                        }
                    }
                }
            }
            // Prepend trace so it appears before the command's own stderr
            result.stderr = format!("{trace}{}", result.stderr);
        }

        // 8. Apply output redirections
        apply_output_redirects(&redirects, &mut result, state)?;

        // 8b. Execute deferred write process substitutions from redirects.
        for rps in &redir_proc_subs {
            if matches!(rps.kind, ast::ProcessSubstitutionKind::Write) {
                let content = state
                    .fs
                    .read_file(Path::new(&rps.temp_path))
                    .map_err(|e| RustBashError::Execution(e.to_string()))?;
                let stdin_data = String::from_utf8_lossy(&content).to_string();
                let mut sub_state = make_proc_sub_state(state);
                let inner_result = execute_compound_list(rps.list, &mut sub_state, &stdin_data)?;
                state.counters.command_count = sub_state.counters.command_count;
                state.counters.output_size = sub_state.counters.output_size;
                state.proc_sub_counter = sub_state.proc_sub_counter;
                result.stdout.push_str(&inner_result.stdout);
                result.stderr.push_str(&inner_result.stderr);
            }
        }

        // 8c. Execute deferred write process substitutions from prefix/suffix args
        for (inner_list, temp_path) in &deferred_write_subs {
            let content = state
                .fs
                .read_file(Path::new(temp_path))
                .map_err(|e| RustBashError::Execution(e.to_string()))?;
            let stdin_data = String::from_utf8_lossy(&content).to_string();
            let mut sub_state = make_proc_sub_state(state);
            let inner_result = execute_compound_list(inner_list, &mut sub_state, &stdin_data)?;
            state.counters.command_count = sub_state.counters.command_count;
            state.counters.output_size = sub_state.counters.output_size;
            state.proc_sub_counter = sub_state.proc_sub_counter;
            result.stdout.push_str(&inner_result.stdout);
            result.stderr.push_str(&inner_result.stderr);
        }

        if !expansion_stderr.is_empty() {
            result.stderr = format!("{expansion_stderr}{}", result.stderr);
        }

        Ok(result)
    })();

    // 8d. Always clean up process substitution temp files (even on error)
    for temp_path in &proc_sub_temps {
        let _ = state.fs.remove_file(Path::new(temp_path));
    }
    state.proc_sub_prealloc.clear();
    if has_temp_binding_scope {
        state.temp_binding_scopes.pop();
    }

    // 9. Restore pre-command assignments
    for (name, old_value) in saved {
        match old_value {
            Some(var) => {
                state.env.insert(name, var);
            }
            None => {
                state.env.remove(&name);
            }
        }
    }

    // 9b. Prepend any prefix-assignment error messages to the result stderr.
    if let Ok(ref mut r) = inner_result
        && !prefix_stderr.is_empty()
    {
        r.stderr = format!("{prefix_stderr}{}", r.stderr);
    }

    inner_result
}

// ── Compound commands ───────────────────────────────────────────────

fn execute_compound_command(
    compound: &ast::CompoundCommand,
    redirects: Option<&ast::RedirectList>,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let (effective_stdin, uses_fresh_stdin) = if let Some(redir_list) = redirects {
        let redir_refs: Vec<&ast::IoRedirect> = redir_list.0.iter().collect();
        let input_redirects = collect_input_redirects(&redir_refs, state, stdin)?;
        (
            input_redirects
                .get(&0)
                .cloned()
                .unwrap_or_else(|| stdin.to_string()),
            input_redirects.contains_key(&0),
        )
    } else {
        (stdin.to_string(), false)
    };
    let saved_stdin_offset = state.stdin_offset;
    if uses_fresh_stdin {
        state.stdin_offset = 0;
    }

    let mut result = match compound {
        ast::CompoundCommand::IfClause(if_clause) => {
            execute_if(if_clause, state, &effective_stdin)?
        }
        ast::CompoundCommand::ForClause(for_clause) => {
            execute_for(for_clause, state, &effective_stdin)?
        }
        ast::CompoundCommand::WhileClause(wc) => {
            execute_while_until(wc, false, state, &effective_stdin)?
        }
        ast::CompoundCommand::UntilClause(uc) => {
            execute_while_until(uc, true, state, &effective_stdin)?
        }
        ast::CompoundCommand::BraceGroup(bg) => {
            execute_compound_list(&bg.list, state, &effective_stdin)?
        }
        ast::CompoundCommand::Subshell(sub) => {
            execute_subshell(&sub.list, state, &effective_stdin)?
        }
        ast::CompoundCommand::CaseClause(cc) => execute_case(cc, state, &effective_stdin)?,
        ast::CompoundCommand::Arithmetic(arith) => execute_arithmetic(arith, state)?,
        ast::CompoundCommand::ArithmeticForClause(afc) => {
            execute_arithmetic_for(afc, state, &effective_stdin)?
        }
        ast::CompoundCommand::Coprocess(_) => {
            state.stdin_offset = saved_stdin_offset;
            return Err(RustBashError::Execution(
                "coproc is not supported".to_string(),
            ));
        }
    };
    state.stdin_offset = saved_stdin_offset;

    // Drain command-substitution stderr accumulated during expansion
    if !state.pending_cmdsub_stderr.is_empty() {
        let cmdsub_stderr = std::mem::take(&mut state.pending_cmdsub_stderr);
        result.stderr = format!("{cmdsub_stderr}{}", result.stderr);
    }

    // Apply redirections attached to the compound command
    if let Some(redir_list) = redirects {
        let redir_refs: Vec<&ast::IoRedirect> = redir_list.0.iter().collect();
        apply_output_redirects(&redir_refs, &mut result, state)?;
    }

    state.last_exit_code = result.exit_code;
    Ok(result)
}

fn execute_if(
    if_clause: &ast::IfClauseCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let mut result = ExecResult::default();

    // Suppress errexit for condition evaluation
    state.errexit_suppressed += 1;
    let cond = execute_compound_list(&if_clause.condition, state, stdin)?;
    state.errexit_suppressed -= 1;
    result.stdout.push_str(&cond.stdout);
    result.stderr.push_str(&cond.stderr);

    if cond.exit_code == 0 {
        let body = execute_compound_list(&if_clause.then, state, stdin)?;
        result.stdout.push_str(&body.stdout);
        result.stderr.push_str(&body.stderr);
        result.exit_code = body.exit_code;
        return Ok(result);
    }

    // Evaluate elif/else branches
    if let Some(elses) = &if_clause.elses {
        for else_clause in elses {
            if let Some(condition) = &else_clause.condition {
                // elif — suppress errexit for condition
                state.errexit_suppressed += 1;
                let cond = execute_compound_list(condition, state, stdin)?;
                state.errexit_suppressed -= 1;
                result.stdout.push_str(&cond.stdout);
                result.stderr.push_str(&cond.stderr);
                if cond.exit_code == 0 {
                    let body = execute_compound_list(&else_clause.body, state, stdin)?;
                    result.stdout.push_str(&body.stdout);
                    result.stderr.push_str(&body.stderr);
                    result.exit_code = body.exit_code;
                    return Ok(result);
                }
            } else {
                // else
                let body = execute_compound_list(&else_clause.body, state, stdin)?;
                result.stdout.push_str(&body.stdout);
                result.stderr.push_str(&body.stderr);
                result.exit_code = body.exit_code;
                return Ok(result);
            }
        }
    }

    // No branch matched — exit code 0 per POSIX
    result.exit_code = 0;
    Ok(result)
}

fn execute_for(
    for_clause: &ast::ForClauseCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    use crate::interpreter::ControlFlow;

    let mut result = ExecResult::default();

    // Bash reports an error when the variable name is invalid
    // (e.g. `for i.j in a b c` → "not a valid identifier").
    let var_name = &for_clause.variable_name;
    if !var_name.is_empty()
        && (!var_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
            || var_name.starts_with(|c: char| c.is_ascii_digit()))
    {
        result.stderr = format!("rust-bash: `{var_name}': not a valid identifier\n");
        result.exit_code = 1;
        state.last_exit_code = 1;
        return Ok(result);
    }

    let values: Vec<String> = if let Some(words) = &for_clause.values {
        let mut vals = Vec::new();
        for w in words {
            vals.extend(expand_word_mut(w, state)?);
        }
        vals
    } else {
        // No word list → iterate over positional parameters
        state.positional_params.clone()
    };

    state.loop_depth += 1;
    let mut iterations: usize = 0;
    for val in &values {
        if state.should_exit {
            break;
        }
        iterations += 1;
        if iterations > state.limits.max_loop_iterations {
            state.loop_depth -= 1;
            return Err(RustBashError::LimitExceeded {
                limit_name: "max_loop_iterations",
                limit_value: state.limits.max_loop_iterations,
                actual_value: iterations,
            });
        }

        set_variable(state, &for_clause.variable_name, val.clone())?;
        let r = execute_compound_list(&for_clause.body.list, state, stdin)?;
        result.stdout.push_str(&r.stdout);
        result.stderr.push_str(&r.stderr);
        result.exit_code = r.exit_code;

        match state.control_flow.take() {
            Some(ControlFlow::Break(n)) => {
                if n > 1 {
                    state.control_flow = Some(ControlFlow::Break(n - 1));
                }
                break;
            }
            Some(ControlFlow::Continue(n)) => {
                if n > 1 {
                    state.control_flow = Some(ControlFlow::Continue(n - 1));
                    break;
                }
                // n == 1: skip rest, continue to next iteration
            }
            Some(ret @ ControlFlow::Return(_)) => {
                state.control_flow = Some(ret);
                break;
            }
            None => {}
        }
    }
    state.loop_depth -= 1;

    Ok(result)
}

fn execute_arithmetic(
    arith: &ast::ArithmeticCommand,
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if arithmetic_command_has_invalid_hash(arith, state) {
        return Ok(ExecResult {
            stderr: "rust-bash: arithmetic: unexpected character `#`\n".to_string(),
            exit_code: 1,
            ..Default::default()
        });
    }

    let expanded =
        crate::interpreter::expansion::expand_arith_expression(&arith.expr.value, state)?;
    match eval_command_arithmetic_expr(&expanded, state) {
        Ok(val) => {
            let mut result = ExecResult {
                exit_code: if val != 0 { 0 } else { 1 },
                ..Default::default()
            };
            if state.shell_opts.xtrace {
                let ps4 = expand_ps4(state);
                result.stderr = format!(
                    "{ps4}(({}))\n{}",
                    arith.expr.value.trim_end(),
                    result.stderr
                );
            }
            Ok(result)
        }
        Err(RustBashError::Execution(msg)) if msg.contains("unbound variable") => {
            // nounset errors are fatal
            Err(RustBashError::Execution(msg))
        }
        Err(e) => {
            // Non-fatal arithmetic error: set exit code 1 and continue
            Ok(ExecResult {
                stderr: format!("rust-bash: {e}\n"),
                exit_code: 1,
                ..Default::default()
            })
        }
    }
}

fn arithmetic_command_has_invalid_hash(
    arith: &ast::ArithmeticCommand,
    state: &InterpreterState,
) -> bool {
    let source = &state.current_source_text;
    if source.is_empty() {
        return false;
    }

    let Some(raw_command) = char_range_slice(source, arith.loc.start.index, arith.loc.end.index)
    else {
        return false;
    };

    raw_arithmetic_command_contains_invalid_hash(raw_command)
}

fn char_range_slice(source: &str, start: usize, end: usize) -> Option<&str> {
    if start > end {
        return None;
    }

    let total_chars = source.chars().count();
    if end > total_chars {
        return None;
    }

    let start_byte = if start == total_chars {
        source.len()
    } else {
        source.char_indices().nth(start)?.0
    };
    let end_byte = if end == total_chars {
        source.len()
    } else {
        source.char_indices().nth(end)?.0
    };
    source.get(start_byte..end_byte)
}

fn raw_arithmetic_command_contains_invalid_hash(raw_command: &str) -> bool {
    let Some(start) = raw_command.find("((") else {
        return false;
    };
    let Some(end) = raw_command.rfind("))") else {
        return false;
    };
    if end <= start + 2 {
        return false;
    }

    let inner = &raw_command[start + 2..end];
    let chars: Vec<char> = inner.chars().collect();
    let mut i = 0usize;
    let mut in_single = false;
    let mut in_double = false;

    while i < chars.len() {
        let ch = chars[i];
        if in_single {
            if ch == '\'' {
                in_single = false;
            }
            i += 1;
            continue;
        }
        if in_double {
            if ch == '\\' {
                i += 2;
                continue;
            }
            if ch == '"' {
                in_double = false;
            }
            i += 1;
            continue;
        }

        match ch {
            '\'' => in_single = true,
            '"' => in_double = true,
            '\\' => {
                i += 2;
                continue;
            }
            '#' => {
                if !is_base_literal_hash(&chars, i) {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }

    false
}

fn is_base_literal_hash(chars: &[char], hash_index: usize) -> bool {
    if hash_index == 0 || hash_index + 1 >= chars.len() {
        return false;
    }

    let next = chars[hash_index + 1];
    if !(next.is_ascii_alphanumeric() || matches!(next, '@' | '_')) {
        return false;
    }

    let mut digit_start = hash_index;
    while digit_start > 0 && chars[digit_start - 1].is_ascii_digit() {
        digit_start -= 1;
    }
    if digit_start == hash_index {
        return false;
    }

    if digit_start > 0 {
        let prev = chars[digit_start - 1];
        if prev.is_ascii_alphanumeric() || prev == '_' {
            return false;
        }
    }

    true
}

fn eval_command_arithmetic_expr(
    expr: &str,
    state: &mut InterpreterState,
) -> Result<i64, RustBashError> {
    let normalized = normalize_command_arithmetic_expr(expr);
    crate::interpreter::arithmetic::eval_arithmetic(&normalized, state)
}

fn normalize_command_arithmetic_expr(expr: &str) -> String {
    if !expr.contains('\'') {
        return expr.to_string();
    }

    let mut normalized = String::with_capacity(expr.len());
    let mut chars = expr.chars();
    let mut in_double = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                in_double = !in_double;
                normalized.push(ch);
            }
            '\\' if in_double => {
                normalized.push(ch);
                if let Some(next) = chars.next() {
                    normalized.push(next);
                }
            }
            '\'' if !in_double => {
                for inner in chars.by_ref() {
                    if inner == '\'' {
                        break;
                    }
                    normalized.push(inner);
                }
            }
            _ => normalized.push(ch),
        }
    }

    normalized
}

fn execute_arithmetic_for(
    afc: &ast::ArithmeticForClauseCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    use crate::interpreter::ControlFlow;

    // Evaluate initializer
    if let Some(init) = &afc.initializer {
        let expanded = crate::interpreter::expansion::expand_arith_expression(&init.value, state)?;
        eval_command_arithmetic_expr(&expanded, state)?;
    }

    let mut result = ExecResult::default();
    let mut iterations: usize = 0;

    state.loop_depth += 1;
    loop {
        if state.should_exit {
            break;
        }
        iterations += 1;
        if iterations > state.limits.max_loop_iterations {
            state.loop_depth -= 1;
            return Err(RustBashError::LimitExceeded {
                limit_name: "max_loop_iterations",
                limit_value: state.limits.max_loop_iterations,
                actual_value: iterations,
            });
        }

        // Evaluate condition (empty condition = always true)
        if let Some(cond) = &afc.condition {
            let expanded =
                crate::interpreter::expansion::expand_arith_expression(&cond.value, state)?;
            let val = eval_command_arithmetic_expr(&expanded, state)?;
            if val == 0 {
                break;
            }
        }

        // Execute body
        let body = execute_compound_list(&afc.body.list, state, stdin)?;
        result.stdout.push_str(&body.stdout);
        result.stderr.push_str(&body.stderr);
        result.exit_code = body.exit_code;

        match state.control_flow.take() {
            Some(ControlFlow::Break(n)) => {
                if n > 1 {
                    state.control_flow = Some(ControlFlow::Break(n - 1));
                }
                break;
            }
            Some(ControlFlow::Continue(n)) => {
                if n > 1 {
                    state.control_flow = Some(ControlFlow::Continue(n - 1));
                    break;
                }
            }
            Some(ret @ ControlFlow::Return(_)) => {
                state.control_flow = Some(ret);
                break;
            }
            None => {}
        }

        // Evaluate updater
        if let Some(upd) = &afc.updater {
            let expanded =
                crate::interpreter::expansion::expand_arith_expression(&upd.value, state)?;
            eval_command_arithmetic_expr(&expanded, state)?;
        }
    }
    state.loop_depth -= 1;

    Ok(result)
}

fn execute_while_until(
    clause: &ast::WhileOrUntilClauseCommand,
    is_until: bool,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    use crate::interpreter::ControlFlow;

    let mut result = ExecResult::default();
    let mut iterations: usize = 0;

    state.loop_depth += 1;
    loop {
        if state.should_exit {
            break;
        }
        iterations += 1;
        if iterations > state.limits.max_loop_iterations {
            state.loop_depth -= 1;
            return Err(RustBashError::LimitExceeded {
                limit_name: "max_loop_iterations",
                limit_value: state.limits.max_loop_iterations,
                actual_value: iterations,
            });
        }

        // Suppress errexit for the loop condition
        state.errexit_suppressed += 1;
        let cond = execute_compound_list(&clause.0, state, stdin)?;
        state.errexit_suppressed -= 1;
        result.stdout.push_str(&cond.stdout);
        result.stderr.push_str(&cond.stderr);

        let should_continue = if is_until {
            cond.exit_code != 0
        } else {
            cond.exit_code == 0
        };

        if !should_continue {
            break;
        }

        let body = execute_compound_list(&clause.1.list, state, stdin)?;
        result.stdout.push_str(&body.stdout);
        result.stderr.push_str(&body.stderr);
        result.exit_code = body.exit_code;

        match state.control_flow.take() {
            Some(ControlFlow::Break(n)) => {
                if n > 1 {
                    state.control_flow = Some(ControlFlow::Break(n - 1));
                }
                break;
            }
            Some(ControlFlow::Continue(n)) => {
                if n > 1 {
                    state.control_flow = Some(ControlFlow::Continue(n - 1));
                    break;
                }
                // n == 1: skip rest, continue to next iteration
            }
            Some(ret @ ControlFlow::Return(_)) => {
                state.control_flow = Some(ret);
                break;
            }
            None => {}
        }
    }
    state.loop_depth -= 1;

    Ok(result)
}

fn execute_subshell(
    list: &ast::CompoundList,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    // Deep-clone the filesystem so mutations in the subshell are isolated
    let cloned_fs = state.fs.deep_clone();

    let mut sub_state = InterpreterState {
        fs: cloned_fs,
        env: state.env.clone(),
        cwd: state.cwd.clone(),
        functions: state.functions.clone(),
        last_exit_code: state.last_exit_code,
        commands: clone_commands(&state.commands),
        shell_opts: state.shell_opts.clone(),
        shopt_opts: state.shopt_opts.clone(),
        limits: state.limits.clone(),
        counters: ExecutionCounters {
            command_count: state.counters.command_count,
            output_size: state.counters.output_size,
            start_time: state.counters.start_time,
            substitution_depth: state.counters.substitution_depth,
            call_depth: 0,
        },
        network_policy: state.network_policy.clone(),
        should_exit: false,
        loop_depth: 0,
        control_flow: None,
        positional_params: state.positional_params.clone(),
        shell_name: state.shell_name.clone(),
        random_seed: state.random_seed,
        local_scopes: Vec::new(),
        temp_binding_scopes: Vec::new(),
        in_function_depth: 0,
        traps: state.traps.clone(),
        in_trap: false,
        errexit_suppressed: 0,
        stdin_offset: 0,
        dir_stack: state.dir_stack.clone(),
        command_hash: state.command_hash.clone(),
        aliases: state.aliases.clone(),
        current_lineno: state.current_lineno,
        current_source: state.current_source.clone(),
        current_source_text: state.current_source_text.clone(),
        shell_start_time: state.shell_start_time,
        last_argument: state.last_argument.clone(),
        call_stack: state.call_stack.clone(),
        machtype: state.machtype.clone(),
        hosttype: state.hosttype.clone(),
        persistent_fds: state.persistent_fds.clone(),
        next_auto_fd: state.next_auto_fd,
        proc_sub_counter: state.proc_sub_counter,
        proc_sub_prealloc: HashMap::new(),
        pipe_stdin_bytes: None,
        pending_cmdsub_stderr: String::new(),
        fatal_expansion_error: false,
    };

    let result = execute_compound_list(list, &mut sub_state, stdin);

    // Fold shared counters back into parent
    state.counters.command_count = sub_state.counters.command_count;
    state.counters.output_size = sub_state.counters.output_size;

    let result = result?;

    // Only the exit code propagates back; all other state changes are discarded
    Ok(result)
}

fn execute_case(
    case_clause: &ast::CaseClauseCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let value = expand_word_to_string_mut(&case_clause.value, state)?;
    let mut result = ExecResult::default();

    let mut i = 0;
    let mut fall_through = false;
    while i < case_clause.cases.len() {
        let case_item = &case_clause.cases[i];

        let matched = if fall_through {
            fall_through = false;
            true
        } else {
            let mut m = false;
            for pattern_word in &case_item.patterns {
                let pattern = expand_word_to_string_mut(pattern_word, state)?;
                let matched_pattern = if state.shopt_opts.nocasematch {
                    if state.shopt_opts.extglob {
                        crate::interpreter::pattern::extglob_match_nocase(&pattern, &value)
                    } else {
                        crate::interpreter::pattern::glob_match_nocase(&pattern, &value)
                    }
                } else if state.shopt_opts.extglob {
                    crate::interpreter::pattern::extglob_match(&pattern, &value)
                } else {
                    crate::interpreter::pattern::glob_match(&pattern, &value)
                };
                if matched_pattern {
                    m = true;
                    break;
                }
            }
            m
        };

        if matched {
            if let Some(cmd) = &case_item.cmd {
                let r = execute_compound_list(cmd, state, stdin)?;
                result.stdout.push_str(&r.stdout);
                result.stderr.push_str(&r.stderr);
                result.exit_code = r.exit_code;
            }

            match case_item.post_action {
                ast::CaseItemPostAction::ExitCase => break,
                ast::CaseItemPostAction::UnconditionallyExecuteNextCaseItem => {
                    // ;& — fall through: execute next body unconditionally
                    fall_through = true;
                    i += 1;
                    continue;
                }
                ast::CaseItemPostAction::ContinueEvaluatingCases => {
                    // ;;& — continue matching remaining patterns
                    i += 1;
                    continue;
                }
            }
        }
        i += 1;
    }

    Ok(result)
}

/// Clone the command registry for subshell isolation.
///
/// Uses `Arc<dyn VirtualCommand>` so that custom commands registered via the
/// public API are preserved in subshells and command substitutions.
pub(crate) fn clone_commands(
    commands: &HashMap<String, Arc<dyn crate::commands::VirtualCommand>>,
) -> HashMap<String, Arc<dyn crate::commands::VirtualCommand>> {
    commands.clone()
}

/// Create an exec callback that commands can use to invoke sub-commands.
/// The callback parses and executes a command string in an isolated subshell state.
///
/// Note: The callback captures `start_time` so wall-clock limits apply globally.
/// Per-invocation `command_count` resets because the `Fn` closure signature cannot
/// fold counters back to the parent. The parent's `dispatch_command` still counts
/// the top-level command (e.g., `xargs`/`find`) itself.
fn make_exec_callback(
    state: &InterpreterState,
) -> impl Fn(&str) -> Result<CommandResult, RustBashError> {
    let cloned_fs = state.fs.deep_clone();
    let env = state.env.clone();
    let cwd = state.cwd.clone();
    let functions = state.functions.clone();
    let last_exit_code = state.last_exit_code;
    let commands = clone_commands(&state.commands);
    let shell_opts = state.shell_opts.clone();
    let shopt_opts = state.shopt_opts.clone();
    let limits = state.limits.clone();
    let network_policy = state.network_policy.clone();
    let positional_params = state.positional_params.clone();
    let shell_name = state.shell_name.clone();
    let random_seed = state.random_seed;
    let start_time = state.counters.start_time;
    let shell_start_time = state.shell_start_time;
    let last_argument = state.last_argument.clone();
    let call_stack = state.call_stack.clone();
    let current_source = state.current_source.clone();
    let current_source_text = state.current_source_text.clone();
    let machtype = state.machtype.clone();
    let hosttype = state.hosttype.clone();

    move |cmd_str: &str| {
        let program = parse(cmd_str)?;

        let sub_fs = cloned_fs.deep_clone();

        let mut sub_state = InterpreterState {
            fs: sub_fs,
            env: env.clone(),
            cwd: cwd.clone(),
            functions: functions.clone(),
            last_exit_code,
            commands: clone_commands(&commands),
            shell_opts: shell_opts.clone(),
            shopt_opts: shopt_opts.clone(),
            limits: limits.clone(),
            counters: ExecutionCounters {
                command_count: 0,
                output_size: 0,
                start_time,
                substitution_depth: 0,
                call_depth: 0,
            },
            network_policy: network_policy.clone(),
            should_exit: false,
            loop_depth: 0,
            control_flow: None,
            positional_params: positional_params.clone(),
            shell_name: shell_name.clone(),
            random_seed,
            local_scopes: Vec::new(),
            temp_binding_scopes: Vec::new(),
            in_function_depth: 0,
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
            stdin_offset: 0,
            dir_stack: Vec::new(),
            command_hash: HashMap::new(),
            aliases: HashMap::new(),
            current_lineno: 0,
            current_source: current_source.clone(),
            current_source_text: current_source_text.clone(),
            shell_start_time,
            last_argument: last_argument.clone(),
            call_stack: call_stack.clone(),
            machtype: machtype.clone(),
            hosttype: hosttype.clone(),
            persistent_fds: HashMap::new(),
            next_auto_fd: 10,
            proc_sub_counter: 0,
            proc_sub_prealloc: HashMap::new(),
            pipe_stdin_bytes: None,
            pending_cmdsub_stderr: String::new(),
            fatal_expansion_error: false,
        };

        let result = execute_program(&program, &mut sub_state)?;
        Ok(CommandResult {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
            stdout_bytes: None,
        })
    }
}

// ── Function calls ──────────────────────────────────────────────────

fn execute_function_call(
    name: &str,
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    use crate::interpreter::ControlFlow;

    // Check call depth limit
    state.counters.call_depth += 1;
    if state.counters.call_depth > state.limits.max_call_depth {
        let actual = state.counters.call_depth;
        state.counters.call_depth -= 1;
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_call_depth",
            limit_value: state.limits.max_call_depth,
            actual_value: actual,
        });
    }

    // Clone the function body so we don't hold a borrow on state.functions
    let func_def = state.functions.get(name).unwrap().clone();

    // Save and replace positional parameters
    let saved_params = std::mem::replace(&mut state.positional_params, args.to_vec());

    // Push call stack frame for FUNCNAME/BASH_SOURCE/BASH_LINENO.
    // BASH_LINENO records the line where the call was made (current LINENO).
    state.call_stack.push(CallFrame {
        func_name: name.to_string(),
        source: func_def.source.clone(),
        lineno: state.current_lineno,
    });

    // Push a new local scope for this function call
    state.local_scopes.push(HashMap::new());
    state.in_function_depth += 1;

    // Execute the function body (CompoundCommand inside FunctionBody)
    let saved_source = std::mem::replace(&mut state.current_source, func_def.source.clone());
    let saved_source_text =
        std::mem::replace(&mut state.current_source_text, func_def.source_text.clone());
    let result = execute_compound_command(&func_def.body.0, func_def.body.1.as_ref(), state, "");
    state.current_source = saved_source;
    state.current_source_text = saved_source_text;

    // Determine exit code: if Return was signaled, use its code
    let exit_code = match state.control_flow.take() {
        Some(ControlFlow::Return(code)) => code,
        Some(other) => {
            // Re-set non-return control flow (break/continue should propagate)
            state.control_flow = Some(other);
            result.as_ref().map(|r| r.exit_code).unwrap_or(1)
        }
        None => result.as_ref().map(|r| r.exit_code).unwrap_or(1),
    };

    // Pop the call stack frame.
    state.call_stack.pop();

    // Restore local variables
    state.in_function_depth -= 1;
    if let Some(restore_map) = state.local_scopes.pop() {
        for (var_name, old_value) in restore_map {
            match old_value {
                Some(var) => {
                    state.env.insert(var_name, var);
                }
                None => {
                    state.env.remove(&var_name);
                }
            }
        }
    }

    // Restore positional parameters
    state.positional_params = saved_params;

    state.counters.call_depth -= 1;

    let mut result = result?;
    result.exit_code = exit_code;
    Ok(result)
}

fn dispatch_command(
    name: &str,
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
    extra_input_fds: Option<&HashMap<i32, String>>,
) -> Result<ExecResult, RustBashError> {
    state.counters.command_count += 1;
    check_limits(state)?;

    // 0. --help interception (before builtin dispatch, function lookup, etc.)
    if args.first().map(|a| a.as_str()) == Some("--help") {
        // Check builtins first
        if let Some(meta) = builtins::builtin_meta(name)
            && meta.supports_help_flag
        {
            return Ok(ExecResult {
                stdout: crate::commands::format_help(meta),
                stderr: String::new(),
                exit_code: 0,
                stdout_bytes: None,
            });
        }
        // Check registered commands
        if let Some(cmd) = state.commands.get(name)
            && let Some(meta) = cmd.meta()
            && meta.supports_help_flag
        {
            return Ok(ExecResult {
                stdout: crate::commands::format_help(meta),
                stderr: String::new(),
                exit_code: 0,
                stdout_bytes: None,
            });
        }
        // No meta or supports_help_flag == false → fall through to normal dispatch
    }

    // 1. Special shell builtins (unshadowable)
    if let Some(result) = builtins::execute_builtin(name, args, state, stdin)? {
        return Ok(result);
    }

    // 2. User-defined functions
    if state.functions.contains_key(name) {
        return execute_function_call(name, args, state);
    }

    // 3. Registered commands
    if let Some(cmd) = state.commands.get(name) {
        let mut env: HashMap<String, String> = state
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.value.as_scalar().to_string()))
            .collect();
        if let Some(extra_fds) = extra_input_fds {
            for (fd, contents) in extra_fds {
                if *fd == 0 {
                    continue;
                }
                env.insert(format!("__RUST_BASH_FD_{fd}"), contents.clone());
            }
        }
        // Clone variables for `test -v` array element checks (before mutable borrow).
        let vars_clone = state.env.clone();
        let fs = Arc::clone(&state.fs);
        let cwd = state.cwd.clone();
        let limits = state.limits.clone();
        let network_policy = state.network_policy.clone();

        // Take binary pipe data from interpreter state before borrowing state for callback
        let binary_stdin = state.pipe_stdin_bytes.take();
        let exec_callback = make_exec_callback(state);

        let ctx = CommandContext {
            fs: &*fs,
            cwd: &cwd,
            env: &env,
            variables: Some(&vars_clone),
            stdin,
            stdin_bytes: binary_stdin.as_deref(),
            limits: &limits,
            network_policy: &network_policy,
            exec: Some(&exec_callback),
            shell_opts: Some(&state.shell_opts),
        };

        // xpg_echo: when enabled, echo interprets backslash escapes by default
        let effective_args: Vec<String>;
        let cmd_args: &[String] = if name == "echo" && state.shopt_opts.xpg_echo {
            effective_args = std::iter::once("-e".to_string())
                .chain(args.iter().cloned())
                .collect();
            &effective_args
        } else {
            args
        };

        let cmd_result = cmd.execute(cmd_args, &ctx);
        return Ok(ExecResult {
            stdout: cmd_result.stdout,
            stderr: cmd_result.stderr,
            exit_code: cmd_result.exit_code,
            stdout_bytes: cmd_result.stdout_bytes,
        });
    }

    // 4. Command not found
    Ok(ExecResult {
        stdout: String::new(),
        stderr: format!("{name}: command not found\n"),
        exit_code: 127,
        stdout_bytes: None,
    })
}

// ── exec builtin ────────────────────────────────────────────────────

/// Extract a `{varname}` FD allocation prefix from command args.
/// Returns `Some(varname)` if the first arg matches `{identifier}`.
fn extract_fd_varname(arg: &str) -> Option<&str> {
    let trimmed = arg.strip_prefix('{')?.strip_suffix('}')?;
    if !trimmed.is_empty()
        && trimmed
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        Some(trimmed)
    } else {
        None
    }
}

/// Handle the `exec` builtin which has three modes:
/// 1. `exec` with only redirects → persistent FD redirections
/// 2. `exec {varname}>file` → FD variable allocation (persistent)
/// 3. `exec cmd args` → replace shell with command
fn execute_exec_builtin(
    args: &[String],
    redirects: &[&ast::IoRedirect],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    // Check for {varname} FD allocation syntax: first arg is {name}, rest is empty
    if let Some(first_arg) = args.first()
        && let Some(varname) = extract_fd_varname(first_arg)
    {
        return exec_fd_variable_alloc(varname, args.get(1..), redirects, state);
    }

    // No real args → persistent FD redirections
    if args.is_empty() {
        return exec_persistent_redirects(redirects, state);
    }

    // Has command args → execute and exit
    let input_redirects = match collect_input_redirects(redirects, state, stdin) {
        Ok(map) => map,
        Err(RustBashError::RedirectFailed(msg)) => {
            let result = ExecResult {
                stderr: format!("rust-bash: {msg}\n"),
                exit_code: 1,
                ..ExecResult::default()
            };
            state.last_exit_code = 1;
            state.should_exit = true;
            return Ok(result);
        }
        Err(e) => return Err(e),
    };
    let effective_stdin = match input_redirects.get(&0) {
        Some(s) => s.clone(),
        None => stdin.to_string(),
    };
    let mut result = dispatch_command(
        &args[0],
        &args[1..],
        state,
        &effective_stdin,
        Some(&input_redirects),
    )?;
    apply_output_redirects(redirects, &mut result, state)?;
    state.last_exit_code = result.exit_code;
    state.should_exit = true;
    Ok(result)
}

/// Apply persistent FD redirections from `exec > file`, `exec 3< file`, etc.
fn exec_persistent_redirects(
    redirects: &[&ast::IoRedirect],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    for redir in redirects {
        match redir {
            ast::IoRedirect::File(fd, kind, target) => {
                let filename = match redirect_target_filename(target, state) {
                    Ok(f) => f,
                    Err(RustBashError::RedirectFailed(msg)) => {
                        return Ok(ExecResult {
                            stderr: format!("rust-bash: {msg}\n"),
                            exit_code: 1,
                            ..ExecResult::default()
                        });
                    }
                    Err(e) => return Err(e),
                };
                let path = resolve_path(&state.cwd, &filename);
                match kind {
                    ast::IoFileRedirectKind::Write | ast::IoFileRedirectKind::Clobber => {
                        let fd_num = fd.unwrap_or(1);
                        if is_dev_null(&path) {
                            state.persistent_fds.insert(fd_num, PersistentFd::DevNull);
                        } else if is_dev_stdout(&path) {
                            // exec > /dev/stdout restores normal stdout
                            state.persistent_fds.remove(&fd_num);
                        } else if is_dev_stderr(&path) {
                            // exec > /dev/stderr is unusual but valid
                            state.persistent_fds.remove(&fd_num);
                        } else {
                            // Create/truncate the file
                            state
                                .fs
                                .write_file(Path::new(&path), b"")
                                .map_err(|e| RustBashError::Execution(e.to_string()))?;
                            state
                                .persistent_fds
                                .insert(fd_num, PersistentFd::OutputFile(path));
                        }
                    }
                    ast::IoFileRedirectKind::Append => {
                        let fd_num = fd.unwrap_or(1);
                        if is_dev_null(&path) {
                            state.persistent_fds.insert(fd_num, PersistentFd::DevNull);
                        } else if is_dev_stdout(&path) || is_dev_stderr(&path) {
                            state.persistent_fds.remove(&fd_num);
                        } else {
                            state
                                .persistent_fds
                                .insert(fd_num, PersistentFd::OutputFile(path));
                        }
                    }
                    ast::IoFileRedirectKind::Read => {
                        let fd_num = fd.unwrap_or(0);
                        if is_dev_null(&path) {
                            state.persistent_fds.insert(fd_num, PersistentFd::DevNull);
                        } else {
                            state
                                .persistent_fds
                                .insert(fd_num, PersistentFd::InputFile(path));
                        }
                    }
                    ast::IoFileRedirectKind::ReadAndWrite => {
                        let fd_num = fd.unwrap_or(0);
                        if !state.fs.exists(Path::new(&path)) {
                            state
                                .fs
                                .write_file(Path::new(&path), b"")
                                .map_err(|e| RustBashError::Execution(e.to_string()))?;
                        }
                        state
                            .persistent_fds
                            .insert(fd_num, PersistentFd::ReadWriteFile(path));
                    }
                    ast::IoFileRedirectKind::DuplicateOutput => {
                        let fd_num = fd.unwrap_or(1);
                        let dup_target = redirect_target_filename(target, state)?;
                        // Handle close: >&-
                        if dup_target == "-" {
                            state.persistent_fds.insert(fd_num, PersistentFd::Closed);
                        } else if let Some(stripped) = dup_target.strip_suffix('-') {
                            // FD move: N>&M-
                            if let Ok(source_fd) = stripped.parse::<i32>() {
                                if let Some(entry) = state.persistent_fds.get(&source_fd).cloned() {
                                    state.persistent_fds.insert(fd_num, entry);
                                }
                                state.persistent_fds.insert(source_fd, PersistentFd::Closed);
                            }
                        } else if let Ok(target_fd) = dup_target.parse::<i32>() {
                            // Dup: N>&M — copy M's destination to N
                            if let Some(entry) = state.persistent_fds.get(&target_fd).cloned() {
                                state.persistent_fds.insert(fd_num, entry);
                            } else if target_fd == 0 || target_fd == 1 || target_fd == 2 {
                                // Standard fd without persistent redirect — store as dup
                                state
                                    .persistent_fds
                                    .insert(fd_num, PersistentFd::DupStdFd(target_fd));
                            } else {
                                state.persistent_fds.remove(&fd_num);
                            }
                        }
                    }
                    ast::IoFileRedirectKind::DuplicateInput => {
                        let fd_num = fd.unwrap_or(0);
                        let dup_target = redirect_target_filename(target, state)?;
                        if dup_target == "-" {
                            state.persistent_fds.insert(fd_num, PersistentFd::Closed);
                        }
                    }
                }
            }
            ast::IoRedirect::OutputAndError(word, _append) => {
                let filename = expand_word_to_string_mut(word, state)?;
                let path = resolve_path(&state.cwd, &filename);
                if is_dev_null(&path) {
                    state.persistent_fds.insert(1, PersistentFd::DevNull);
                    state.persistent_fds.insert(2, PersistentFd::DevNull);
                } else {
                    let pfd = PersistentFd::OutputFile(path);
                    state.persistent_fds.insert(1, pfd.clone());
                    state.persistent_fds.insert(2, pfd);
                }
            }
            _ => {}
        }
    }
    Ok(ExecResult::default())
}

/// Handle `exec {varname}>file` — allocate an FD number and store in variable.
fn exec_fd_variable_alloc(
    varname: &str,
    extra_args: Option<&[String]>,
    redirects: &[&ast::IoRedirect],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    // Check for close syntax: `exec {fd}>&-`
    let is_close = redirects.iter().any(|r| {
        matches!(
            r,
            ast::IoRedirect::File(_, ast::IoFileRedirectKind::DuplicateOutput, ast::IoFileRedirectTarget::Duplicate(w)) if w.value == "-"
        )
    });

    if is_close {
        // Close the FD stored in the variable
        if let Some(var) = state.env.get(varname)
            && let Ok(fd_num) = var.value.as_scalar().parse::<i32>()
        {
            state.persistent_fds.insert(fd_num, PersistentFd::Closed);
        }
        return Ok(ExecResult::default());
    }

    // Check for extra args after {varname} — not supported, but handle gracefully
    if extra_args.is_some_and(|a| !a.is_empty()) {
        return Ok(ExecResult {
            stderr: "rust-bash: exec: too many arguments\n".to_string(),
            exit_code: 1,
            ..Default::default()
        });
    }

    // Allocate a new FD number
    let fd_num = state.next_auto_fd;
    state.next_auto_fd += 1;

    // Store the allocated FD number in the named variable
    set_variable(state, varname, fd_num.to_string())?;

    // Apply the redirect to the allocated FD
    for redir in redirects {
        if let ast::IoRedirect::File(_fd, kind, target) = redir {
            let filename = redirect_target_filename(target, state)?;
            let path = resolve_path(&state.cwd, &filename);
            match kind {
                ast::IoFileRedirectKind::Write | ast::IoFileRedirectKind::Clobber => {
                    if is_dev_null(&path) {
                        state.persistent_fds.insert(fd_num, PersistentFd::DevNull);
                    } else if is_dev_stdout(&path) || is_dev_stderr(&path) {
                        state.persistent_fds.remove(&fd_num);
                    } else {
                        state
                            .fs
                            .write_file(Path::new(&path), b"")
                            .map_err(|e| RustBashError::Execution(e.to_string()))?;
                        state
                            .persistent_fds
                            .insert(fd_num, PersistentFd::OutputFile(path));
                    }
                }
                ast::IoFileRedirectKind::Append => {
                    if is_dev_null(&path) {
                        state.persistent_fds.insert(fd_num, PersistentFd::DevNull);
                    } else if is_dev_stdout(&path) || is_dev_stderr(&path) {
                        state.persistent_fds.remove(&fd_num);
                    } else {
                        state
                            .persistent_fds
                            .insert(fd_num, PersistentFd::OutputFile(path));
                    }
                }
                ast::IoFileRedirectKind::Read => {
                    if is_dev_null(&path) {
                        state.persistent_fds.insert(fd_num, PersistentFd::DevNull);
                    } else {
                        state
                            .persistent_fds
                            .insert(fd_num, PersistentFd::InputFile(path));
                    }
                }
                ast::IoFileRedirectKind::ReadAndWrite => {
                    if is_dev_null(&path) {
                        state.persistent_fds.insert(fd_num, PersistentFd::DevNull);
                    } else {
                        if !state.fs.exists(Path::new(&path)) {
                            state
                                .fs
                                .write_file(Path::new(&path), b"")
                                .map_err(|e| RustBashError::Execution(e.to_string()))?;
                        }
                        state
                            .persistent_fds
                            .insert(fd_num, PersistentFd::ReadWriteFile(path));
                    }
                }
                _ => {}
            }
            break; // Only process the first file redirect
        }
    }

    Ok(ExecResult::default())
}

// ── Special device paths ────────────────────────────────────────────

fn is_dev_stdout(path: &str) -> bool {
    path == "/dev/stdout"
}

fn is_dev_stderr(path: &str) -> bool {
    path == "/dev/stderr"
}

fn is_dev_stdin(path: &str) -> bool {
    path == "/dev/stdin"
}

fn is_dev_zero(path: &str) -> bool {
    path == "/dev/zero"
}

fn is_dev_full(path: &str) -> bool {
    path == "/dev/full"
}

fn is_special_dev_path(path: &str) -> bool {
    is_dev_null(path)
        || is_dev_stdout(path)
        || is_dev_stderr(path)
        || is_dev_stdin(path)
        || is_dev_zero(path)
        || is_dev_full(path)
}

fn expand_heredoc_body(
    heredoc: &ast::IoHereDocument,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    let mut body = if heredoc.requires_expansion {
        let mut expanded_input = String::new();
        let mut chars = heredoc.doc.value.chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.peek().copied() {
                    Some('\n') => {
                        chars.next();
                    }
                    Some('\\') => {
                        chars.next();
                        expanded_input.push_str("__RB_HEREDOC_BSLASH__");
                    }
                    Some('$') => {
                        chars.next();
                        expanded_input.push_str("__RB_HEREDOC_DOLLAR__");
                    }
                    Some('`') => {
                        chars.next();
                        expanded_input.push_str("__RB_HEREDOC_BQUOTE__");
                    }
                    Some('"') => {
                        chars.next();
                        expanded_input.push_str("__RB_HEREDOC_DQ__");
                    }
                    Some(other) => {
                        expanded_input.push('\\');
                        expanded_input.push(other);
                        chars.next();
                    }
                    None => expanded_input.push('\\'),
                }
            } else {
                expanded_input.push(ch);
            }
        }

        let expanded = expand_word_to_string_mut(
            &ast::Word {
                value: expanded_input,
                loc: heredoc.doc.loc.clone(),
            },
            state,
        )?;
        expanded
            .replace("__RB_HEREDOC_BSLASH__", "\\")
            .replace("__RB_HEREDOC_DOLLAR__", "$")
            .replace("__RB_HEREDOC_BQUOTE__", "`")
            .replace("__RB_HEREDOC_DQ__", "\\\"")
    } else {
        heredoc.doc.value.clone()
    };

    if heredoc.remove_tabs {
        body = body
            .lines()
            .map(|l| l.trim_start_matches('\t'))
            .collect::<Vec<_>>()
            .join("\n")
            + if body.ends_with('\n') { "\n" } else { "" };
    }

    if body.len() > state.limits.max_heredoc_size {
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_heredoc_size",
            limit_value: state.limits.max_heredoc_size,
            actual_value: body.len(),
        });
    }

    Ok(body)
}

fn read_input_redirect(
    fd_num: i32,
    redir: &ast::IoRedirect,
    resolved: &HashMap<i32, String>,
    state: &mut InterpreterState,
    default_stdin: &str,
) -> Result<Option<String>, RustBashError> {
    match redir {
        ast::IoRedirect::File(fd, kind, target) => {
            if fd.unwrap_or(0) != fd_num {
                return Ok(None);
            }
            match kind {
                ast::IoFileRedirectKind::Read | ast::IoFileRedirectKind::ReadAndWrite => {
                    let filename = redirect_target_filename(target, state)?;
                    let path = resolve_path(&state.cwd, &filename);
                    if is_dev_stdin(&path) {
                        return Ok(Some(default_stdin.to_string()));
                    }
                    if is_dev_null(&path) || is_dev_zero(&path) || is_dev_full(&path) {
                        return Ok(Some(String::new()));
                    }
                    if filename.is_empty() {
                        return Err(RustBashError::RedirectFailed(
                            ": No such file or directory".to_string(),
                        ));
                    }
                    let content = state.fs.read_file(Path::new(&path)).map_err(|_| {
                        RustBashError::RedirectFailed(format!(
                            "{filename}: No such file or directory"
                        ))
                    })?;
                    Ok(Some(String::from_utf8_lossy(&content).to_string()))
                }
                ast::IoFileRedirectKind::DuplicateInput => {
                    let dup_target = redirect_target_filename(target, state)?;
                    if let Ok(source_fd) = dup_target.parse::<i32>() {
                        if let Some(content) = resolved.get(&source_fd) {
                            return Ok(Some(content.clone()));
                        }
                        if let Some(pfd) = state.persistent_fds.get(&source_fd) {
                            match pfd {
                                PersistentFd::InputFile(path)
                                | PersistentFd::ReadWriteFile(path) => {
                                    let content = state
                                        .fs
                                        .read_file(Path::new(path))
                                        .map_err(|e| RustBashError::Execution(e.to_string()))?;
                                    return Ok(Some(String::from_utf8_lossy(&content).to_string()));
                                }
                                PersistentFd::DevNull | PersistentFd::Closed => {
                                    return Ok(Some(String::new()));
                                }
                                PersistentFd::OutputFile(_) | PersistentFd::DupStdFd(_) => {}
                            }
                        }
                    }
                    Ok(None)
                }
                _ => Ok(None),
            }
        }
        ast::IoRedirect::HereString(fd, word) => {
            if fd.unwrap_or(0) != fd_num {
                return Ok(None);
            }
            let val = expand_word_to_string_mut(word, state)?;
            if val.len() > state.limits.max_heredoc_size {
                return Err(RustBashError::LimitExceeded {
                    limit_name: "max_heredoc_size",
                    limit_value: state.limits.max_heredoc_size,
                    actual_value: val.len(),
                });
            }
            Ok(Some(format!("{val}\n")))
        }
        ast::IoRedirect::HereDocument(fd, heredoc) => {
            if fd.unwrap_or(0) != fd_num {
                return Ok(None);
            }
            Ok(Some(expand_heredoc_body(heredoc, state)?))
        }
        _ => Ok(None),
    }
}

fn collect_input_redirects(
    redirects: &[&ast::IoRedirect],
    state: &mut InterpreterState,
    default_stdin: &str,
) -> Result<HashMap<i32, String>, RustBashError> {
    let mut resolved: HashMap<i32, String> = HashMap::new();
    for redir in redirects {
        let fd_num = match redir {
            ast::IoRedirect::File(
                fd,
                ast::IoFileRedirectKind::Read
                | ast::IoFileRedirectKind::ReadAndWrite
                | ast::IoFileRedirectKind::DuplicateInput,
                _,
            ) => fd.unwrap_or(0),
            ast::IoRedirect::HereString(fd, _) | ast::IoRedirect::HereDocument(fd, _) => {
                fd.unwrap_or(0)
            }
            _ => continue,
        };
        if let Some(content) = read_input_redirect(fd_num, redir, &resolved, state, default_stdin)?
        {
            resolved.insert(fd_num, content);
        }
    }
    Ok(resolved)
}

fn apply_output_redirects(
    redirects: &[&ast::IoRedirect],
    result: &mut ExecResult,
    state: &mut InterpreterState,
) -> Result<(), RustBashError> {
    // Track which FDs have explicit per-command redirects
    let mut redirected_fds = std::collections::HashSet::new();
    // Redirect errors (e.g. /dev/full) bypass the redirect chain — they go to
    // the shell's own stderr, not to the command's possibly-redirected stderr.
    let mut deferred_errors: Vec<String> = Vec::new();

    for redir in redirects {
        match redir {
            ast::IoRedirect::File(fd, kind, target) => {
                let fd_num = match kind {
                    ast::IoFileRedirectKind::Read
                    | ast::IoFileRedirectKind::ReadAndWrite
                    | ast::IoFileRedirectKind::DuplicateInput => fd.unwrap_or(0),
                    _ => fd.unwrap_or(1),
                };
                redirected_fds.insert(fd_num);
                let cont =
                    apply_file_redirect(*fd, kind, target, result, state, &mut deferred_errors)?;
                if !cont {
                    break;
                }
            }
            ast::IoRedirect::OutputAndError(word, append) => {
                redirected_fds.insert(1);
                redirected_fds.insert(2);
                let filename = expand_word_to_string_mut(word, state)?;
                if filename.is_empty() {
                    result
                        .stderr
                        .push_str("rust-bash: : No such file or directory\n");
                    result.exit_code = 1;
                    break;
                }
                let path = resolve_path(&state.cwd, &filename);

                // noclobber: block &> on existing file (append &>> is fine)
                if state.shell_opts.noclobber
                    && !*append
                    && !is_dev_null(&path)
                    && state.fs.exists(Path::new(&path))
                {
                    result.stderr.push_str(&format!(
                        "rust-bash: {filename}: cannot overwrite existing file\n"
                    ));
                    result.stdout.clear();
                    result.exit_code = 1;
                    break;
                }

                let combined = format!("{}{}", result.stdout, result.stderr);

                if is_dev_null(&path) {
                    result.stdout.clear();
                    result.stderr.clear();
                } else if *append {
                    write_or_append(state, &path, &combined, true)?;
                    result.stdout.clear();
                    result.stderr.clear();
                } else {
                    write_or_append(state, &path, &combined, false)?;
                    result.stdout.clear();
                    result.stderr.clear();
                }
            }
            _ => {} // HereString/HereDocument handled in stdin
        }
    }

    // Apply persistent FD redirections for FDs that don't have per-command redirects
    apply_persistent_fd_fallback(result, state, &redirected_fds)?;

    // Append deferred redirect errors to stderr — these bypass the redirect
    // chain, mirroring how bash reports redirect failures on the shell's own
    // stderr (saved before redirect setup).
    for err in deferred_errors {
        result.stderr.push_str(&err);
    }

    Ok(())
}

/// Apply persistent FD redirections as fallback for FDs without per-command redirects.
fn apply_persistent_fd_fallback(
    result: &mut ExecResult,
    state: &InterpreterState,
    redirected_fds: &std::collections::HashSet<i32>,
) -> Result<(), RustBashError> {
    // Check persistent FD for stdout (FD 1)
    if !redirected_fds.contains(&1)
        && let Some(pfd) = state.persistent_fds.get(&1)
    {
        match pfd {
            PersistentFd::OutputFile(path) => {
                if !result.stdout.is_empty() {
                    write_or_append(state, path, &result.stdout, true)?;
                    result.stdout.clear();
                }
            }
            PersistentFd::DevNull | PersistentFd::Closed => {
                result.stdout.clear();
            }
            _ => {}
        }
    }

    // Check persistent FD for stderr (FD 2)
    if !redirected_fds.contains(&2)
        && let Some(pfd) = state.persistent_fds.get(&2)
    {
        match pfd {
            PersistentFd::OutputFile(path) => {
                if !result.stderr.is_empty() {
                    write_or_append(state, path, &result.stderr, true)?;
                    result.stderr.clear();
                }
            }
            PersistentFd::DevNull | PersistentFd::Closed => {
                result.stderr.clear();
            }
            _ => {}
        }
    }

    Ok(())
}

/// Apply a single file redirect. Returns `Ok(true)` to continue processing
/// more redirects, or `Ok(false)` to stop (e.g., noclobber failure).
fn apply_file_redirect(
    fd: Option<i32>,
    kind: &ast::IoFileRedirectKind,
    target: &ast::IoFileRedirectTarget,
    result: &mut ExecResult,
    state: &mut InterpreterState,
    deferred_errors: &mut Vec<String>,
) -> Result<bool, RustBashError> {
    // Helper macro to catch RedirectFailed from redirect_target_filename
    macro_rules! try_filename {
        ($target:expr, $state:expr, $result:expr) => {
            match redirect_target_filename($target, $state) {
                Ok(f) => f,
                Err(RustBashError::RedirectFailed(msg)) => {
                    // Clear output for the redirected fd (since no file to write to)
                    let fd_num = fd.unwrap_or(1);
                    if fd_num == 1 {
                        $result.stdout.clear();
                    } else if fd_num == 2 {
                        $result.stderr.clear();
                    }
                    $result.stderr.push_str(&format!("rust-bash: {msg}\n"));
                    $result.exit_code = 1;
                    return Ok(false);
                }
                Err(e) => return Err(e),
            }
        };
    }

    match kind {
        ast::IoFileRedirectKind::Write | ast::IoFileRedirectKind::Clobber => {
            let fd_num = fd.unwrap_or(1);
            let filename = try_filename!(target, state, result);
            let path = resolve_path(&state.cwd, &filename);

            // noclobber: `>` on existing file is an error; `>|` (Clobber) bypasses
            if state.shell_opts.noclobber
                && matches!(kind, ast::IoFileRedirectKind::Write)
                && !is_dev_null(&path)
                && !is_special_dev_path(&path)
                && state.fs.exists(Path::new(&path))
            {
                result.stderr.push_str(&format!(
                    "rust-bash: {filename}: cannot overwrite existing file\n"
                ));
                if fd_num == 1 {
                    result.stdout.clear();
                }
                result.exit_code = 1;
                return Ok(false);
            }

            apply_write_redirect(fd_num, &path, result, state, false, deferred_errors)?;
        }
        ast::IoFileRedirectKind::Append => {
            let fd_num = fd.unwrap_or(1);
            let filename = try_filename!(target, state, result);
            let path = resolve_path(&state.cwd, &filename);
            apply_write_redirect(fd_num, &path, result, state, true, deferred_errors)?;
        }
        ast::IoFileRedirectKind::DuplicateOutput => {
            let fd_num = fd.unwrap_or(1);
            if !apply_duplicate_output(fd_num, target, result, state)? {
                return Ok(false);
            }
        }
        ast::IoFileRedirectKind::DuplicateInput => {
            let fd_num = fd.unwrap_or(0);
            if fd_num == 0 {
                // <&N for stdin — handled in collect_input_redirects
            } else {
                // N<&M where N != 0 — acts like N>&M (duplicate)
                if !apply_duplicate_output(fd_num, target, result, state)? {
                    return Ok(false);
                }
            }
        }
        ast::IoFileRedirectKind::Read => {
            // Handled in collect_input_redirects
        }
        ast::IoFileRedirectKind::ReadAndWrite => {
            let fd_num = fd.unwrap_or(0);
            let filename = try_filename!(target, state, result);
            let path = resolve_path(&state.cwd, &filename);
            if !state.fs.exists(Path::new(&path)) {
                state
                    .fs
                    .write_file(Path::new(&path), b"")
                    .map_err(|e| RustBashError::Execution(e.to_string()))?;
            }
            // For FD 0, input is handled in collect_input_redirects.
            // For output FDs, write content to the file.
            if fd_num == 1 {
                write_or_append(state, &path, &result.stdout, false)?;
                result.stdout.clear();
            } else if fd_num == 2 {
                write_or_append(state, &path, &result.stderr, false)?;
                result.stderr.clear();
            }
        }
    }
    Ok(true)
}

/// Apply a write/append redirect for a given FD to a path, handling special devices.
fn apply_write_redirect(
    fd_num: i32,
    path: &str,
    result: &mut ExecResult,
    state: &InterpreterState,
    append: bool,
    deferred_errors: &mut Vec<String>,
) -> Result<(), RustBashError> {
    if is_dev_null(path) || is_dev_zero(path) {
        if fd_num == 1 {
            result.stdout.clear();
            result.stdout_bytes = None;
        } else if fd_num == 2 {
            result.stderr.clear();
        }
    } else if is_dev_stdout(path) {
        // > /dev/stdout → output stays on stdout (no-op for fd 1)
        if fd_num == 2 {
            result.stdout.push_str(&result.stderr);
            result.stderr.clear();
        }
    } else if is_dev_stderr(path) {
        // > /dev/stderr → output goes to stderr
        if fd_num == 1 {
            result.stderr.push_str(&result.stdout);
            result.stdout.clear();
        }
    } else if is_dev_full(path) {
        // Writing to /dev/full always fails with ENOSPC.
        // The error goes to the shell's own stderr (deferred), mirroring how
        // bash reports redirect failures on the pre-redirect stderr.
        deferred_errors
            .push("rust-bash: write error: /dev/full: No space left on device\n".to_string());
        if fd_num == 1 {
            result.stdout.clear();
        } else if fd_num == 2 {
            result.stderr.clear();
        }
        result.exit_code = 1;
    } else {
        // Check if path is a directory — redirect to directory should fail gracefully
        let p = Path::new(path);
        if state.fs.exists(p)
            && let Ok(meta) = state.fs.stat(p)
            && meta.node_type == crate::vfs::NodeType::Directory
        {
            let basename = path.rsplit('/').next().unwrap_or(path);
            let display = if basename.is_empty() { path } else { basename };
            deferred_errors.push(format!("rust-bash: {display}: Is a directory\n"));
            if fd_num == 1 {
                result.stdout.clear();
            } else if fd_num == 2 {
                result.stderr.clear();
            }
            result.exit_code = 1;
            return Ok(());
        }
        let content_bytes: Vec<u8> = if fd_num == 1 {
            // Prefer binary bytes when available (e.g. gzip output)
            if let Some(bytes) = result.stdout_bytes.take() {
                bytes
            } else {
                result.stdout.as_bytes().to_vec()
            }
        } else if fd_num == 2 {
            result.stderr.as_bytes().to_vec()
        } else {
            return write_to_persistent_fd(fd_num, result, state);
        };
        write_or_append_bytes(state, path, &content_bytes, append)?;
        if fd_num == 1 {
            result.stdout.clear();
            result.stdout_bytes = None;
        } else if fd_num == 2 {
            result.stderr.clear();
        }
    }
    Ok(())
}

/// Write to a persistent FD's target file (for higher-numbered FDs like >&10).
fn write_to_persistent_fd(
    _fd_num: i32,
    _result: &mut ExecResult,
    _state: &InterpreterState,
) -> Result<(), RustBashError> {
    // Higher FDs with no persistent mapping are silently ignored
    Ok(())
}

/// Handle DuplicateOutput redirect (>&N, 2>&1, N>&M-, etc.)
/// Returns Ok(true) to continue, Ok(false) if the redirect failed.
fn apply_duplicate_output(
    fd_num: i32,
    target: &ast::IoFileRedirectTarget,
    result: &mut ExecResult,
    state: &mut InterpreterState,
) -> Result<bool, RustBashError> {
    let dup_target_str = match target {
        ast::IoFileRedirectTarget::Duplicate(word) => expand_word_to_string_mut(word, state)?,
        ast::IoFileRedirectTarget::Fd(target_fd) => target_fd.to_string(),
        _ => return Ok(true),
    };

    // Handle close: >&-
    if dup_target_str == "-" {
        if fd_num == 1 {
            result.stdout.clear();
        } else if fd_num == 2 {
            result.stderr.clear();
        }
        return Ok(true);
    }

    // Handle FD move: N>&M-
    if let Some(source_str) = dup_target_str.strip_suffix('-') {
        if let Ok(source_fd) = source_str.parse::<i32>() {
            // Duplicate: copy source to dest
            apply_dup_fd(fd_num, source_fd, result, state)?;
            // Close source
            if source_fd == 1 {
                result.stdout.clear();
            } else if source_fd == 2 {
                result.stderr.clear();
            } else {
                state.persistent_fds.insert(source_fd, PersistentFd::Closed);
            }
        }
        return Ok(true);
    }

    // Standard duplication: N>&M
    if let Ok(target_fd) = dup_target_str.parse::<i32>() {
        // Validate the target FD exists (0=stdin, 1=stdout, 2=stderr, or a persistent fd)
        if target_fd != 0
            && target_fd != 1
            && target_fd != 2
            && !state.persistent_fds.contains_key(&target_fd)
        {
            if fd_num == 1 {
                result.stdout.clear();
            }
            result
                .stderr
                .push_str(&format!("rust-bash: {fd_num}: Bad file descriptor\n"));
            result.exit_code = 1;
            return Ok(false);
        }
        apply_dup_fd(fd_num, target_fd, result, state)?;
    }
    Ok(true)
}

/// Duplicate target_fd to fd_num in the result streams.
fn apply_dup_fd(
    fd_num: i32,
    target_fd: i32,
    result: &mut ExecResult,
    state: &InterpreterState,
) -> Result<(), RustBashError> {
    // Standard FD duplication
    if target_fd == 1 && fd_num == 2 {
        // 2>&1: merge stderr into stdout
        result.stdout.push_str(&result.stderr);
        result.stderr.clear();
    } else if target_fd == 2 && fd_num == 1 {
        // 1>&2: merge stdout into stderr
        result.stderr.push_str(&result.stdout);
        result.stdout.clear();
    } else if fd_num == 1 || fd_num == 2 {
        // Redirect stdout/stderr to a persistent FD target
        if let Some(pfd) = state.persistent_fds.get(&target_fd) {
            match pfd {
                PersistentFd::OutputFile(path) => {
                    let content = if fd_num == 1 {
                        let c = result.stdout.clone();
                        result.stdout.clear();
                        c
                    } else {
                        let c = result.stderr.clone();
                        result.stderr.clear();
                        c
                    };
                    write_or_append(state, path, &content, true)?;
                }
                PersistentFd::DevNull | PersistentFd::Closed => {
                    if fd_num == 1 {
                        result.stdout.clear();
                    } else {
                        result.stderr.clear();
                    }
                }
                PersistentFd::DupStdFd(std_fd) => {
                    // Redirect fd_num to the standard fd that target_fd points to
                    if *std_fd == 1 && fd_num == 2 {
                        result.stdout.push_str(&result.stderr);
                        result.stderr.clear();
                    } else if *std_fd == 2 && fd_num == 1 {
                        result.stderr.push_str(&result.stdout);
                        result.stdout.clear();
                    }
                    // fd_num == *std_fd is a no-op (already going there)
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Handle a process substitution in a command prefix or suffix.
/// For `<(cmd)`: execute inner command and write stdout to a temp file.
/// For `>(cmd)`: create empty temp file and record inner command for deferred execution.
/// Returns the temp file path to use as a command argument.
fn expand_process_substitution<'a>(
    kind: &ast::ProcessSubstitutionKind,
    list: &'a ast::CompoundList,
    state: &mut InterpreterState,
    deferred_write_subs: &mut Vec<(&'a ast::CompoundList, String)>,
) -> Result<String, RustBashError> {
    match kind {
        ast::ProcessSubstitutionKind::Read => execute_read_process_substitution(list, state),
        ast::ProcessSubstitutionKind::Write => {
            let path = allocate_proc_sub_temp_file(state, b"")?;
            deferred_write_subs.push((list, path.clone()));
            Ok(path)
        }
    }
}

fn redirect_target_filename(
    target: &ast::IoFileRedirectTarget,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    match target {
        ast::IoFileRedirectTarget::Filename(word) => {
            let filename = expand_word_to_string_mut(word, state)?;
            if filename.is_empty() {
                return Err(RustBashError::RedirectFailed(
                    ": No such file or directory".to_string(),
                ));
            }
            Ok(filename)
        }
        ast::IoFileRedirectTarget::Fd(fd) => Ok(fd.to_string()),
        ast::IoFileRedirectTarget::Duplicate(word) => expand_word_to_string_mut(word, state),
        // Only valid when called from within execute_simple_command's closure,
        // where proc_sub_prealloc has been populated in step 5b.
        ast::IoFileRedirectTarget::ProcessSubstitution(_, _) => {
            // Look up pre-allocated path by AST-node address (populated in execute_simple_command).
            let key = std::ptr::from_ref(target) as usize;
            state.proc_sub_prealloc.remove(&key).ok_or_else(|| {
                RustBashError::Execution(
                    "process substitution: no pre-allocated path available".into(),
                )
            })
        }
    }
}

/// Execute a `<(cmd)` process substitution: run the inner command, capture stdout,
/// write to a temp VFS file, and return the temp file path.
fn execute_read_process_substitution(
    list: &ast::CompoundList,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    let mut sub_state = make_proc_sub_state(state);
    let result = execute_compound_list(list, &mut sub_state, "")?;

    // Fold shared counters back
    state.counters.command_count = sub_state.counters.command_count;
    state.counters.output_size = sub_state.counters.output_size;
    state.proc_sub_counter = sub_state.proc_sub_counter;

    allocate_proc_sub_temp_file(state, result.stdout.as_bytes())
}

/// Allocate a unique temp VFS file with the given content and return its path.
fn allocate_proc_sub_temp_file(
    state: &mut InterpreterState,
    content: &[u8],
) -> Result<String, RustBashError> {
    let path = format!("/tmp/.proc_sub_{}", state.proc_sub_counter);
    state.proc_sub_counter += 1;

    // Ensure /tmp exists
    let tmp = Path::new("/tmp");
    if !state.fs.exists(tmp) {
        state
            .fs
            .mkdir_p(tmp)
            .map_err(|e| RustBashError::Execution(e.to_string()))?;
    }

    state
        .fs
        .write_file(Path::new(&path), content)
        .map_err(|e| RustBashError::Execution(e.to_string()))?;

    Ok(path)
}

/// Create a subshell `InterpreterState` that shares the parent's filesystem.
/// Unlike command substitution which deep-clones the fs, process substitution
/// needs the temp file to be visible to the outer command.
fn make_proc_sub_state(state: &mut InterpreterState) -> InterpreterState {
    InterpreterState {
        fs: Arc::clone(&state.fs),
        env: state.env.clone(),
        cwd: state.cwd.clone(),
        functions: state.functions.clone(),
        last_exit_code: state.last_exit_code,
        commands: clone_commands(&state.commands),
        shell_opts: state.shell_opts.clone(),
        shopt_opts: state.shopt_opts.clone(),
        limits: state.limits.clone(),
        counters: ExecutionCounters {
            command_count: state.counters.command_count,
            output_size: state.counters.output_size,
            start_time: state.counters.start_time,
            substitution_depth: state.counters.substitution_depth,
            call_depth: 0,
        },
        network_policy: state.network_policy.clone(),
        should_exit: false,
        loop_depth: 0,
        control_flow: None,
        positional_params: state.positional_params.clone(),
        shell_name: state.shell_name.clone(),
        random_seed: state.random_seed,
        local_scopes: Vec::new(),
        temp_binding_scopes: Vec::new(),
        in_function_depth: 0,
        traps: HashMap::new(),
        in_trap: false,
        errexit_suppressed: 0,
        stdin_offset: 0,
        dir_stack: state.dir_stack.clone(),
        command_hash: state.command_hash.clone(),
        aliases: state.aliases.clone(),
        current_lineno: state.current_lineno,
        current_source: state.current_source.clone(),
        current_source_text: state.current_source_text.clone(),
        shell_start_time: state.shell_start_time,
        last_argument: state.last_argument.clone(),
        call_stack: state.call_stack.clone(),
        machtype: state.machtype.clone(),
        hosttype: state.hosttype.clone(),
        persistent_fds: HashMap::new(),
        next_auto_fd: 10,
        proc_sub_counter: state.proc_sub_counter,
        proc_sub_prealloc: HashMap::new(),
        pipe_stdin_bytes: None,
        pending_cmdsub_stderr: String::new(),
        fatal_expansion_error: false,
    }
}

fn is_dev_null(path: &str) -> bool {
    path == "/dev/null"
}

fn write_or_append(
    state: &InterpreterState,
    path: &str,
    content: &str,
    append: bool,
) -> Result<(), RustBashError> {
    write_or_append_bytes(state, path, content.as_bytes(), append)
}

fn write_or_append_bytes(
    state: &InterpreterState,
    path: &str,
    content: &[u8],
    append: bool,
) -> Result<(), RustBashError> {
    let p = Path::new(path);

    if append {
        if state.fs.exists(p) {
            state
                .fs
                .append_file(p, content)
                .map_err(|e| RustBashError::Execution(e.to_string()))?;
        } else {
            state
                .fs
                .write_file(p, content)
                .map_err(|e| RustBashError::Execution(e.to_string()))?;
        }
    } else {
        state
            .fs
            .write_file(p, content)
            .map_err(|e| RustBashError::Execution(e.to_string()))?;
    }
    Ok(())
}

// ── Extended test ([[ ]]) ──────────────────────────────────────────

fn execute_extended_test(
    expr: &ast::ExtendedTestExpr,
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let should_trace = state.shell_opts.xtrace;
    let mut exec_result = match eval_extended_test_expr(expr, state) {
        Ok(result) => ExecResult {
            exit_code: if result { 0 } else { 1 },
            ..ExecResult::default()
        },
        Err(RustBashError::Execution(ref msg)) => {
            let exit_code = if msg.contains("invalid regex") { 2 } else { 1 };
            state.last_exit_code = exit_code;
            ExecResult {
                stderr: format!("rust-bash: {msg}\n"),
                exit_code,
                ..ExecResult::default()
            }
        }
        Err(e) => return Err(e),
    };
    if should_trace {
        let repr = format_extended_test_expr_expanded(expr, state);
        let ps4 = expand_ps4(state);
        exec_result.stderr = format!("{ps4}[[ {repr} ]]\n{}", exec_result.stderr);
    }
    Ok(exec_result)
}

/// Format an extended test expression for xtrace output, expanding variables.
fn format_extended_test_expr_expanded(
    expr: &ast::ExtendedTestExpr,
    state: &mut InterpreterState,
) -> String {
    match expr {
        ast::ExtendedTestExpr::And(l, r) => {
            format!(
                "{} && {}",
                format_extended_test_expr_expanded(l, state),
                format_extended_test_expr_expanded(r, state)
            )
        }
        ast::ExtendedTestExpr::Or(l, r) => {
            format!(
                "{} || {}",
                format_extended_test_expr_expanded(l, state),
                format_extended_test_expr_expanded(r, state)
            )
        }
        ast::ExtendedTestExpr::Not(inner) => {
            format!("! {}", format_extended_test_expr_expanded(inner, state))
        }
        ast::ExtendedTestExpr::Parenthesized(inner) => {
            format_extended_test_expr_expanded(inner, state)
        }
        ast::ExtendedTestExpr::UnaryTest(pred, word) => {
            let expanded = expand_word_to_string_mut(word, state).unwrap_or_default();
            format!("{} {}", format_unary_pred(pred), expanded)
        }
        ast::ExtendedTestExpr::BinaryTest(pred, l, r) => {
            let l_exp = expand_word_to_string_mut(l, state).unwrap_or_default();
            let r_exp = expand_word_to_string_mut(r, state).unwrap_or_default();
            format!("{} {} {}", l_exp, format_binary_pred(pred), r_exp)
        }
    }
}

fn format_unary_pred(pred: &ast::UnaryPredicate) -> &'static str {
    use brush_parser::ast::UnaryPredicate;
    match pred {
        UnaryPredicate::FileExists => "-a",
        UnaryPredicate::FileExistsAndIsBlockSpecialFile => "-b",
        UnaryPredicate::FileExistsAndIsCharSpecialFile => "-c",
        UnaryPredicate::FileExistsAndIsDir => "-d",
        UnaryPredicate::FileExistsAndIsRegularFile => "-f",
        UnaryPredicate::FileExistsAndIsSetgid => "-g",
        UnaryPredicate::FileExistsAndIsSymlink => "-h",
        UnaryPredicate::FileExistsAndHasStickyBit => "-k",
        UnaryPredicate::FileExistsAndIsFifo => "-p",
        UnaryPredicate::FileExistsAndIsReadable => "-r",
        UnaryPredicate::FileExistsAndIsNotZeroLength => "-s",
        UnaryPredicate::FdIsOpenTerminal => "-t",
        UnaryPredicate::FileExistsAndIsSetuid => "-u",
        UnaryPredicate::FileExistsAndIsWritable => "-w",
        UnaryPredicate::FileExistsAndIsExecutable => "-x",
        UnaryPredicate::FileExistsAndOwnedByEffectiveGroupId => "-G",
        UnaryPredicate::FileExistsAndModifiedSinceLastRead => "-N",
        UnaryPredicate::FileExistsAndOwnedByEffectiveUserId => "-O",
        UnaryPredicate::FileExistsAndIsSocket => "-S",
        UnaryPredicate::StringHasZeroLength => "-z",
        UnaryPredicate::StringHasNonZeroLength => "-n",
        UnaryPredicate::ShellOptionEnabled => "-o",
        UnaryPredicate::ShellVariableIsSetAndAssigned => "-v",
        UnaryPredicate::ShellVariableIsSetAndNameRef => "-R",
    }
}

fn format_binary_pred(pred: &ast::BinaryPredicate) -> &'static str {
    use brush_parser::ast::BinaryPredicate;
    match pred {
        BinaryPredicate::StringExactlyMatchesPattern => "==",
        BinaryPredicate::StringDoesNotExactlyMatchPattern => "!=",
        BinaryPredicate::StringExactlyMatchesString => "==",
        BinaryPredicate::StringDoesNotExactlyMatchString => "!=",
        BinaryPredicate::StringMatchesRegex => "=~",
        BinaryPredicate::StringContainsSubstring => "=~",
        BinaryPredicate::ArithmeticEqualTo => "-eq",
        BinaryPredicate::ArithmeticNotEqualTo => "-ne",
        BinaryPredicate::ArithmeticLessThan => "-lt",
        BinaryPredicate::ArithmeticGreaterThan => "-gt",
        BinaryPredicate::ArithmeticLessThanOrEqualTo => "-le",
        BinaryPredicate::ArithmeticGreaterThanOrEqualTo => "-ge",
        BinaryPredicate::FilesReferToSameDeviceAndInodeNumbers => "-ef",
        BinaryPredicate::LeftFileIsNewerOrExistsWhenRightDoesNot => "-nt",
        BinaryPredicate::LeftFileIsOlderOrDoesNotExistWhenRightDoes => "-ot",
        _ => "?",
    }
}

fn eval_extended_test_expr(
    expr: &ast::ExtendedTestExpr,
    state: &mut InterpreterState,
) -> Result<bool, RustBashError> {
    match expr {
        ast::ExtendedTestExpr::And(left, right) => {
            let l = eval_extended_test_expr(left, state)?;
            if !l {
                return Ok(false);
            }
            eval_extended_test_expr(right, state)
        }
        ast::ExtendedTestExpr::Or(left, right) => {
            let l = eval_extended_test_expr(left, state)?;
            if l {
                return Ok(true);
            }
            eval_extended_test_expr(right, state)
        }
        ast::ExtendedTestExpr::Not(inner) => {
            let val = eval_extended_test_expr(inner, state)?;
            Ok(!val)
        }
        ast::ExtendedTestExpr::Parenthesized(inner) => eval_extended_test_expr(inner, state),
        ast::ExtendedTestExpr::UnaryTest(pred, word) => {
            use brush_parser::ast::UnaryPredicate;
            // Handle -v specially: we need access to full interpreter state for array elements
            if matches!(pred, UnaryPredicate::ShellVariableIsSetAndAssigned) {
                let operand = expand_word_to_string_mut(word, state)?;
                return Ok(test_variable_is_set(&operand, state));
            }
            let operand = expand_word_to_string_mut(word, state)?;
            let env: HashMap<String, String> = state
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.value.as_scalar().to_string()))
                .collect();
            Ok(crate::commands::test_cmd::eval_unary_predicate(
                pred,
                &operand,
                &*state.fs,
                &state.cwd,
                &env,
                Some(&state.shell_opts),
            ))
        }
        ast::ExtendedTestExpr::BinaryTest(pred, left_word, right_word) => {
            let left = expand_word_to_string_mut(left_word, state)?;

            // Regex matching needs special handling
            if matches!(
                pred,
                ast::BinaryPredicate::StringMatchesRegex
                    | ast::BinaryPredicate::StringContainsSubstring
            ) {
                // For =~, if the pattern is entirely quoted, treat as literal string match.
                // In bash, [[ str =~ 'pat' ]] uses literal matching, not regex.
                let raw = &right_word.value;
                let is_fully_quoted = is_word_fully_quoted(raw);
                let pattern = expand_word_to_string_mut(right_word, state)?;
                if is_fully_quoted {
                    return Ok(left.contains(&pattern));
                }
                // Check for partial quoting — extract quoted portions and escape them
                let effective_pattern = build_regex_with_quoted_literals(raw, state)?;
                return eval_regex_match(&left, &effective_pattern, state);
            }

            let right = expand_word_to_string_mut(right_word, state)?;

            // Arithmetic predicates (-eq, -ne, -lt, -gt, -le, -ge) evaluate
            // operands as arithmetic expressions in [[ ]] context.
            // Try parse_bash_int first (handles octal, hex, base-N), then
            // fall back to simple_arith_eval for expressions like "1+2".
            use brush_parser::ast::BinaryPredicate;
            if matches!(
                pred,
                BinaryPredicate::ArithmeticEqualTo
                    | BinaryPredicate::ArithmeticNotEqualTo
                    | BinaryPredicate::ArithmeticLessThan
                    | BinaryPredicate::ArithmeticGreaterThan
                    | BinaryPredicate::ArithmeticLessThanOrEqualTo
                    | BinaryPredicate::ArithmeticGreaterThanOrEqualTo
            ) {
                let lval =
                    crate::commands::test_cmd::parse_bash_int_pub(&left).unwrap_or_else(|| {
                        crate::interpreter::arithmetic::eval_arithmetic(&left, state).unwrap_or(0)
                    });
                let rval =
                    crate::commands::test_cmd::parse_bash_int_pub(&right).unwrap_or_else(|| {
                        crate::interpreter::arithmetic::eval_arithmetic(&right, state).unwrap_or(0)
                    });
                let result = match pred {
                    BinaryPredicate::ArithmeticEqualTo => lval == rval,
                    BinaryPredicate::ArithmeticNotEqualTo => lval != rval,
                    BinaryPredicate::ArithmeticLessThan => lval < rval,
                    BinaryPredicate::ArithmeticGreaterThan => lval > rval,
                    BinaryPredicate::ArithmeticLessThanOrEqualTo => lval <= rval,
                    BinaryPredicate::ArithmeticGreaterThanOrEqualTo => lval >= rval,
                    _ => unreachable!(),
                };
                return Ok(result);
            }

            // Pattern matching (glob) for == and != inside [[
            // Extglob is always active in [[ ]] pattern context
            if state.shopt_opts.nocasematch {
                // Case-insensitive pattern matching
                let result = match pred {
                    ast::BinaryPredicate::StringExactlyMatchesPattern => {
                        crate::interpreter::pattern::extglob_match_nocase(&right, &left)
                    }
                    ast::BinaryPredicate::StringDoesNotExactlyMatchPattern => {
                        !crate::interpreter::pattern::extglob_match_nocase(&right, &left)
                    }
                    ast::BinaryPredicate::StringExactlyMatchesString => {
                        left.eq_ignore_ascii_case(&right)
                    }
                    ast::BinaryPredicate::StringDoesNotExactlyMatchString => {
                        !left.eq_ignore_ascii_case(&right)
                    }
                    _ => crate::commands::test_cmd::eval_binary_predicate(
                        pred, &left, &right, true, &*state.fs, &state.cwd,
                    ),
                };
                Ok(result)
            } else {
                // Use extglob-aware matching for pattern predicates
                let result = match pred {
                    ast::BinaryPredicate::StringExactlyMatchesPattern => {
                        crate::interpreter::pattern::extglob_match(&right, &left)
                    }
                    ast::BinaryPredicate::StringDoesNotExactlyMatchPattern => {
                        !crate::interpreter::pattern::extglob_match(&right, &left)
                    }
                    _ => crate::commands::test_cmd::eval_binary_predicate(
                        pred, &left, &right, true, &*state.fs, &state.cwd,
                    ),
                };
                Ok(result)
            }
        }
    }
}

/// Check if a variable (possibly an array element) is set.
/// Handles `a[i]` syntax for array element checks.
fn test_variable_is_set(operand: &str, state: &mut InterpreterState) -> bool {
    // Check for array subscript syntax: name[index]
    if let Some(bracket_pos) = operand.find('[')
        && operand.ends_with(']')
    {
        let name = &operand[..bracket_pos];
        let index = &operand[bracket_pos + 1..operand.len() - 1];
        let resolved = crate::interpreter::resolve_nameref_or_self(name, state);

        if index == "@" || index == "*" {
            return state
                .env
                .get(&resolved)
                .is_some_and(|var| match &var.value {
                    VariableValue::IndexedArray(map) => !map.is_empty(),
                    VariableValue::AssociativeArray(map) => !map.is_empty(),
                    _ => false,
                });
        }

        // Determine variable type before evaluating arithmetic.
        let var_type = state.env.get(&resolved).map(|var| match &var.value {
            VariableValue::IndexedArray(_) => 0,
            VariableValue::AssociativeArray(_) => 1,
            VariableValue::Scalar(_) => 2,
        });

        return match var_type {
            Some(0) => {
                // Indexed array: evaluate index as arithmetic.
                let idx = eval_index_arithmetic(index, state);
                let Some(var) = state.env.get(&resolved) else {
                    return false;
                };
                if let VariableValue::IndexedArray(map) = &var.value {
                    let actual_idx = if idx < 0 {
                        let max_key = map.keys().next_back().copied().unwrap_or(0);
                        let resolved_idx = max_key as i64 + 1 + idx;
                        if resolved_idx < 0 {
                            return false;
                        }
                        resolved_idx as usize
                    } else {
                        idx as usize
                    };
                    map.contains_key(&actual_idx)
                } else {
                    false
                }
            }
            Some(1) => {
                // Associative array: use index as string key.
                state
                    .env
                    .get(&resolved)
                    .and_then(|var| {
                        if let VariableValue::AssociativeArray(map) = &var.value {
                            Some(map.contains_key(index))
                        } else {
                            None
                        }
                    })
                    .unwrap_or(false)
            }
            Some(2) => {
                // Scalar: index 0 or -1 means it's set.
                let idx = eval_index_arithmetic(index, state);
                idx == 0 || idx == -1
            }
            _ => false,
        };
    }
    // Plain variable name
    let resolved = crate::interpreter::resolve_nameref_or_self(operand, state);
    state.env.contains_key(&resolved)
}

/// Evaluate an array index expression using full arithmetic evaluation.
/// Falls back to simple_arith_eval on errors.
fn eval_index_arithmetic(index: &str, state: &mut InterpreterState) -> i64 {
    crate::interpreter::arithmetic::eval_arithmetic(index, state)
        .unwrap_or_else(|_| crate::interpreter::expansion::simple_arith_eval(index, state))
}

fn eval_regex_match(
    string: &str,
    pattern: &str,
    state: &mut InterpreterState,
) -> Result<bool, RustBashError> {
    // When nocasematch is on, prepend (?i) to make the regex case-insensitive
    let effective_pattern = if state.shopt_opts.nocasematch {
        format!("(?i){pattern}")
    } else {
        pattern.to_string()
    };
    let re = regex::Regex::new(&effective_pattern)
        .map_err(|e| RustBashError::Execution(format!("invalid regex '{pattern}': {e}")))?;

    if let Some(captures) = re.captures(string) {
        // Store BASH_REMATCH as a proper indexed array:
        // index 0 = whole match, index 1..N = capture groups
        let mut map = std::collections::BTreeMap::new();
        let whole = captures.get(0).map(|m| m.as_str()).unwrap_or("");
        map.insert(0, whole.to_string());
        for i in 1..captures.len() {
            let val = captures.get(i).map(|m| m.as_str()).unwrap_or("");
            map.insert(i, val.to_string());
        }
        state.env.insert(
            "BASH_REMATCH".to_string(),
            Variable {
                value: VariableValue::IndexedArray(map),
                attrs: VariableAttrs::empty(),
            },
        );
        Ok(true)
    } else {
        // Clear BASH_REMATCH on non-match
        state.env.insert(
            "BASH_REMATCH".to_string(),
            Variable {
                value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            },
        );
        Ok(false)
    }
}

/// Check if a raw word value is entirely wrapped in quotes.
fn is_word_fully_quoted(raw: &str) -> bool {
    let trimmed = raw.trim();
    if trimmed.len() < 2 {
        return false;
    }
    // Single quotes: 'content'
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') {
        return true;
    }
    // Double quotes: "content"
    if trimmed.starts_with('"') && trimmed.ends_with('"') {
        return true;
    }
    // $'content' or $"content"
    if (trimmed.starts_with("$'") && trimmed.ends_with('\''))
        || (trimmed.starts_with("$\"") && trimmed.ends_with('"'))
    {
        return true;
    }
    false
}

/// Build a regex pattern from a raw word, escaping quoted portions as literals.
/// In bash, quoted parts of a regex pattern are treated as literal text.
fn build_regex_with_quoted_literals(
    raw: &str,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    let mut result = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\'' => {
                // Single-quoted: everything until next ' is literal
                i += 1;
                let mut literal = String::new();
                while i < chars.len() && chars[i] != '\'' {
                    literal.push(chars[i]);
                    i += 1;
                }
                if i < chars.len() {
                    i += 1; // skip closing '
                }
                result.push_str(&regex::escape(&literal));
            }
            '"' => {
                // Double-quoted: until matching " (expand variables inside)
                i += 1;
                let mut content = String::new();
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        content.push(chars[i + 1]);
                        i += 2;
                    } else {
                        content.push(chars[i]);
                        i += 1;
                    }
                }
                if i < chars.len() {
                    i += 1; // skip closing "
                }
                // Expand variables in the content
                let word = ast::Word {
                    value: content,
                    loc: None,
                };
                let expanded = expand_word_to_string_mut(&word, state)?;
                result.push_str(&regex::escape(&expanded));
            }
            '\\' if i + 1 < chars.len() => {
                // Escaped character: treat as literal
                result.push_str(&regex::escape(&chars[i + 1].to_string()));
                i += 2;
            }
            '$' => {
                // Variable expansion — expand and keep as regex
                let mut var_text = String::new();
                var_text.push('$');
                i += 1;
                if i < chars.len() && chars[i] == '{' {
                    // ${...}
                    var_text.push('{');
                    i += 1;
                    let mut depth = 1;
                    while i < chars.len() && depth > 0 {
                        if chars[i] == '{' {
                            depth += 1;
                        } else if chars[i] == '}' {
                            depth -= 1;
                        }
                        var_text.push(chars[i]);
                        i += 1;
                    }
                } else {
                    // $name
                    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                        var_text.push(chars[i]);
                        i += 1;
                    }
                }
                let word = ast::Word {
                    value: var_text,
                    loc: None,
                };
                let expanded = expand_word_to_string_mut(&word, state)?;
                result.push_str(&expanded);
            }
            c => {
                result.push(c);
                i += 1;
            }
        }
    }
    Ok(result)
}
