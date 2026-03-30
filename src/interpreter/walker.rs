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
        let r = execute_and_or_list(and_or_list, state, stdin)?;
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
    let mut pipe_data = stdin.to_string();
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
        let r = execute_command(command, state, &pipe_data)?;
        pipe_data = r.stdout;
        combined_stderr.push_str(&r.stderr);
        exit_code = r.exit_code;
        exit_codes.push(r.exit_code);
    }

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

    Ok(ExecResult {
        stdout: pipe_data,
        stderr: combined_stderr,
        exit_code,
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
                        },
                    );
                    Ok(ExecResult::default())
                }
                Err(e) => Err(e),
            }
        }
        ast::Command::ExtendedTest(ext_test) => execute_extended_test(&ext_test.expr, state),
    };

    match result {
        Err(RustBashError::ExpansionError { message, exit_code }) => {
            state.last_exit_code = exit_code;
            state.should_exit = true;
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
        elements: Vec<(Option<usize>, String)>,
    },
    /// `name[index]=value` — single array element
    ArrayElement {
        name: String,
        index: String,
        value: String,
    },
    /// `name+=(val1 val2 ...)` — append to array
    AppendArray {
        name: String,
        elements: Vec<(Option<usize>, String)>,
    },
    /// `name+=value` — append to scalar
    AppendScalar { name: String, value: String },
}

impl Assignment {
    fn name(&self) -> &str {
        match self {
            Assignment::Scalar { name, .. }
            | Assignment::IndexedArray { name, .. }
            | Assignment::ArrayElement { name, .. }
            | Assignment::AppendArray { name, .. }
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
            let mut elements = Vec::new();
            for (opt_idx_word, val_word) in items {
                let idx = if let Some(idx_word) = opt_idx_word {
                    let idx_str = expand_word_to_string_mut(idx_word, state)?;
                    let idx_val = crate::interpreter::arithmetic::eval_arithmetic(&idx_str, state)?;
                    if idx_val < 0 {
                        return Err(RustBashError::Execution(format!(
                            "negative array subscript: {idx_val}"
                        )));
                    }
                    Some(idx_val as usize)
                } else {
                    None
                };
                let val = expand_word_to_string_mut(val_word, state)?;
                elements.push((idx, val));
            }
            if append {
                Ok(Assignment::AppendArray {
                    name: name.clone(),
                    elements,
                })
            } else {
                Ok(Assignment::IndexedArray {
                    name: name.clone(),
                    elements,
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
            Ok(Assignment::ArrayElement {
                name: name.clone(),
                index: expanded_index,
                value,
            })
        }
        (ast::AssignmentName::ArrayElementName(name, _), ast::AssignmentValue::Array(_)) => Err(
            RustBashError::Execution(format!("{name}: cannot assign array to array element")),
        ),
    }
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
        Assignment::IndexedArray { name, elements } => {
            if let Some(var) = state.env.get(&name)
                && var.readonly()
            {
                return Err(RustBashError::Execution(format!(
                    "{name}: readonly variable"
                )));
            }
            let limit = state.limits.max_array_elements;
            let mut map = std::collections::BTreeMap::new();
            let mut auto_idx: usize = 0;
            for (opt_idx, val) in elements {
                let idx = opt_idx.unwrap_or(auto_idx);
                if map.len() >= limit {
                    return Err(RustBashError::LimitExceeded {
                        limit_name: "max_array_elements",
                        limit_value: limit,
                        actual_value: map.len() + 1,
                    });
                }
                map.insert(idx, val);
                auto_idx = idx + 1;
            }
            let attrs = state
                .env
                .get(&name)
                .map(|v| v.attrs)
                .unwrap_or(VariableAttrs::empty());
            state.env.insert(
                name,
                Variable {
                    value: VariableValue::IndexedArray(map),
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
                if idx < 0 {
                    return Err(RustBashError::Execution(format!(
                        "negative array subscript: {idx}"
                    )));
                }
                set_array_element(state, &name, idx as usize, value)?;
            }
        }
        Assignment::AppendArray { name, elements } => {
            // Find current max index + 1
            let start_idx = match state.env.get(&name) {
                Some(var) => match &var.value {
                    VariableValue::IndexedArray(map) => {
                        map.keys().next_back().map(|k| k + 1).unwrap_or(0)
                    }
                    VariableValue::Scalar(s) if s.is_empty() => 0,
                    VariableValue::Scalar(_) => 1,
                    VariableValue::AssociativeArray(_) => 0,
                },
                None => 0,
            };

            // If the variable doesn't exist yet, create it
            if !state.env.contains_key(&name) {
                state.env.insert(
                    name.clone(),
                    Variable {
                        value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                        attrs: VariableAttrs::empty(),
                    },
                );
            }

            // Convert scalar to array if needed
            if let Some(var) = state.env.get_mut(&name)
                && let VariableValue::Scalar(s) = &var.value
            {
                let mut map = std::collections::BTreeMap::new();
                if !s.is_empty() {
                    map.insert(0, s.clone());
                }
                var.value = VariableValue::IndexedArray(map);
            }

            let mut auto_idx = start_idx;
            for (opt_idx, val) in elements {
                let idx = opt_idx.unwrap_or(auto_idx);
                set_array_element(state, &name, idx, val)?;
                auto_idx = idx + 1;
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

fn execute_simple_command(
    cmd: &ast::SimpleCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    // noexec: skip all simple commands except "set"
    if state.shell_opts.noexec {
        let is_set = cmd.word_or_name.as_ref().is_some_and(|w| w.value == "set");
        if !is_set {
            return Ok(ExecResult::default());
        }
    }

    // 1. Collect redirections and assignments from prefix
    let mut assignments: Vec<Assignment> = Vec::new();
    let mut redirects: Vec<&ast::IoRedirect> = Vec::new();
    // Track process substitution temp files for cleanup
    let mut proc_sub_temps: Vec<String> = Vec::new();
    // Track deferred write process substitutions: (inner command list, temp path)
    let mut deferred_write_subs: Vec<(&ast::CompoundList, String)> = Vec::new();

    if let Some(prefix) = &cmd.prefix {
        for item in &prefix.0 {
            match item {
                ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, _word) => {
                    let a = process_assignment(assignment, assignment.append, state)?;
                    assignments.push(a);
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
    let cmd_name = cmd
        .word_or_name
        .as_ref()
        .map(|w| expand_word_to_string_mut(w, state))
        .transpose()?;

    // 3. Expand arguments and collect redirections from suffix
    let mut args: Vec<String> = Vec::new();
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
                ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, _word) => {
                    // For declaration builtins (export, readonly, declare, local),
                    // assignments in suffix are forwarded as "NAME=VALUE" args.
                    let name = match &assignment.name {
                        ast::AssignmentName::VariableName(n) => n.clone(),
                        ast::AssignmentName::ArrayElementName(n, _) => n.clone(),
                    };
                    let value = match &assignment.value {
                        ast::AssignmentValue::Scalar(w) => expand_word_to_string_mut(w, state)?,
                        ast::AssignmentValue::Array(_) => String::new(),
                    };
                    args.push(format!("{name}={value}"));
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
    let Some(cmd_name) = cmd_name else {
        // xtrace for bare assignments (e.g. `X=1`)
        if state.shell_opts.xtrace && !assignments.is_empty() {
            let ps4 = state
                .env
                .get("PS4")
                .map(|v| v.value.as_scalar().to_string())
                .unwrap_or_else(|| "+ ".to_string());
            let trace_parts: Vec<String> = assignments
                .iter()
                .map(|a| match a {
                    Assignment::Scalar { name, value } => format!("{name}={value}"),
                    Assignment::IndexedArray { name, .. } => format!("{name}=(...)"),
                    Assignment::ArrayElement {
                        name, index, value, ..
                    } => format!("{name}[{index}]={value}"),
                    Assignment::AppendArray { name, .. } => format!("{name}+=(...)"),
                    Assignment::AppendScalar { name, value } => format!("{name}+={value}"),
                })
                .collect();
            let trace_line = format!("{}{}\n", ps4, trace_parts.join(" "));
            // Bare assignments produce no output, but emit xtrace to stderr
            let result = ExecResult {
                stderr: trace_line,
                ..ExecResult::default()
            };
            for a in assignments {
                apply_assignment(a, state)?;
            }
            return Ok(result);
        }
        for a in assignments {
            apply_assignment(a, state)?;
        }
        return Ok(ExecResult::default());
    };

    // 4b. Empty command name (e.g. from `$(false)`) → no command, persist assignments
    if cmd_name.is_empty() && args.is_empty() {
        for a in assignments {
            apply_assignment(a, state)?;
        }
        return Ok(ExecResult {
            exit_code: state.last_exit_code,
            ..ExecResult::default()
        });
    }

    // 4c. Alias expansion: if expand_aliases is on and the command name is an alias,
    // substitute the alias value. Multi-word aliases produce a new command + extra args.
    let (cmd_name, args) = if state.shopt_opts.expand_aliases {
        if let Some(expansion) = state.aliases.get(&cmd_name).cloned() {
            let mut parts: Vec<String> = expansion
                .split_whitespace()
                .map(|s| s.to_string())
                .collect();
            if parts.is_empty() {
                (cmd_name, args)
            } else {
                let new_cmd = parts.remove(0);
                parts.extend(args);
                (new_cmd, parts)
            }
        } else {
            (cmd_name, args)
        }
    } else {
        (cmd_name, args)
    };

    // 4d. Handle `exec` builtin specially — it needs access to redirects.
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
            });
        }
        for a in &assignments {
            apply_assignment(a.clone(), state)?;
        }
        return execute_exec_builtin(&args, &redirects, state, stdin);
    }

    // 5. Apply temporary pre-command assignments
    let mut saved: Vec<(String, Option<Variable>)> = Vec::new();
    for a in &assignments {
        saved.push((a.name().to_string(), state.env.get(a.name()).cloned()));
        apply_assignment(a.clone(), state)?;
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

    let inner_result = (|| -> Result<ExecResult, RustBashError> {
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
        let effective_stdin = get_stdin_from_redirects(&redirects, state, stdin)?;

        // Track last argument for $_ (last argument of the simple command).
        state.last_argument = last_arg.clone();

        // 7a. Capture xtrace state before dispatch (so `set +x` is still traced)
        //     (captured outside closure as `should_trace`)

        // 7. Dispatch command
        let mut result = dispatch_command(&cmd_name, &args, state, &effective_stdin)?;

        // 7b. Emit xtrace to stderr
        if should_trace {
            let ps4 = state
                .env
                .get("PS4")
                .map(|v| v.value.as_scalar().to_string())
                .unwrap_or_else(|| "+ ".to_string());
            let mut trace_words = vec![cmd_name.clone()];
            trace_words.extend(args.iter().cloned());
            let trace_line = format!("{}{}\n", ps4, trace_words.join(" "));
            // Prepend trace so it appears before the command's own stderr
            result.stderr = format!("{trace_line}{}", result.stderr);
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

        Ok(result)
    })();

    // 8d. Always clean up process substitution temp files (even on error)
    for temp_path in &proc_sub_temps {
        let _ = state.fs.remove_file(Path::new(temp_path));
    }
    state.proc_sub_prealloc.clear();

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

    inner_result
}

// ── Compound commands ───────────────────────────────────────────────

fn execute_compound_command(
    compound: &ast::CompoundCommand,
    redirects: Option<&ast::RedirectList>,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let mut result = match compound {
        ast::CompoundCommand::IfClause(if_clause) => execute_if(if_clause, state, stdin)?,
        ast::CompoundCommand::ForClause(for_clause) => execute_for(for_clause, state, stdin)?,
        ast::CompoundCommand::WhileClause(wc) => execute_while_until(wc, false, state, stdin)?,
        ast::CompoundCommand::UntilClause(uc) => execute_while_until(uc, true, state, stdin)?,
        ast::CompoundCommand::BraceGroup(bg) => execute_compound_list(&bg.list, state, stdin)?,
        ast::CompoundCommand::Subshell(sub) => execute_subshell(&sub.list, state, stdin)?,
        ast::CompoundCommand::CaseClause(cc) => execute_case(cc, state, stdin)?,
        ast::CompoundCommand::Arithmetic(arith) => execute_arithmetic(arith, state)?,
        ast::CompoundCommand::ArithmeticForClause(afc) => {
            execute_arithmetic_for(afc, state, stdin)?
        }
    };

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
    let val = crate::interpreter::arithmetic::eval_arithmetic(&arith.expr.value, state)?;
    Ok(ExecResult {
        exit_code: if val != 0 { 0 } else { 1 },
        ..Default::default()
    })
}

fn execute_arithmetic_for(
    afc: &ast::ArithmeticForClauseCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    use crate::interpreter::ControlFlow;

    // Evaluate initializer
    if let Some(init) = &afc.initializer {
        crate::interpreter::arithmetic::eval_arithmetic(&init.value, state)?;
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
            let val = crate::interpreter::arithmetic::eval_arithmetic(&cond.value, state)?;
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
            crate::interpreter::arithmetic::eval_arithmetic(&upd.value, state)?;
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
        in_function_depth: 0,
        traps: state.traps.clone(),
        in_trap: false,
        errexit_suppressed: 0,
        stdin_offset: 0,
        dir_stack: state.dir_stack.clone(),
        command_hash: state.command_hash.clone(),
        aliases: state.aliases.clone(),
        current_lineno: state.current_lineno,
        shell_start_time: state.shell_start_time,
        last_argument: state.last_argument.clone(),
        call_stack: state.call_stack.clone(),
        machtype: state.machtype.clone(),
        hosttype: state.hosttype.clone(),
        persistent_fds: state.persistent_fds.clone(),
        next_auto_fd: state.next_auto_fd,
        proc_sub_counter: state.proc_sub_counter,
        proc_sub_prealloc: HashMap::new(),
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
                    crate::interpreter::pattern::glob_match_nocase(&pattern, &value)
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
/// **Limitation:** Custom commands registered via the public API are not
/// preserved in subshells because `Box<dyn VirtualCommand>` is not `Clone`.
/// Only the default built-in command set is available inside subshells.
/// A future improvement could use `Arc<dyn VirtualCommand>` to share
/// command instances across subshell boundaries.
pub(crate) fn clone_commands(
    _commands: &HashMap<String, Box<dyn crate::commands::VirtualCommand>>,
) -> HashMap<String, Box<dyn crate::commands::VirtualCommand>> {
    crate::commands::register_default_commands()
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
            in_function_depth: 0,
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
            stdin_offset: 0,
            dir_stack: Vec::new(),
            command_hash: HashMap::new(),
            aliases: HashMap::new(),
            current_lineno: 0,
            shell_start_time,
            last_argument: last_argument.clone(),
            call_stack: call_stack.clone(),
            machtype: machtype.clone(),
            hosttype: hosttype.clone(),
            persistent_fds: HashMap::new(),
            next_auto_fd: 10,
            proc_sub_counter: 0,
            proc_sub_prealloc: HashMap::new(),
        };

        let result = execute_program(&program, &mut sub_state)?;
        Ok(CommandResult {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code: result.exit_code,
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
        source: String::new(),
        lineno: state.current_lineno,
    });

    // Push a new local scope for this function call
    state.local_scopes.push(HashMap::new());
    state.in_function_depth += 1;

    // Execute the function body (CompoundCommand inside FunctionBody)
    let result = execute_compound_command(&func_def.body.0, func_def.body.1.as_ref(), state, "");

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
        let env: HashMap<String, String> = state
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.value.as_scalar().to_string()))
            .collect();
        let fs = Arc::clone(&state.fs);
        let cwd = state.cwd.clone();
        let limits = state.limits.clone();
        let network_policy = state.network_policy.clone();

        let exec_callback = make_exec_callback(state);

        let ctx = CommandContext {
            fs: &*fs,
            cwd: &cwd,
            env: &env,
            stdin,
            limits: &limits,
            network_policy: &network_policy,
            exec: Some(&exec_callback),
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
        });
    }

    // 4. Command not found
    Ok(ExecResult {
        stdout: String::new(),
        stderr: format!("{name}: command not found\n"),
        exit_code: 127,
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
    let effective_stdin = get_stdin_from_redirects(redirects, state, stdin)?;
    let mut result = dispatch_command(&args[0], &args[1..], state, &effective_stdin)?;
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
                let filename = redirect_target_filename(target, state)?;
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
                            } else {
                                // Target is a standard FD without persistent redirect — clear
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

fn get_stdin_from_redirects(
    redirects: &[&ast::IoRedirect],
    state: &mut InterpreterState,
    default_stdin: &str,
) -> Result<String, RustBashError> {
    for redir in redirects {
        match redir {
            ast::IoRedirect::File(fd, kind, target) => {
                let fd_num = fd.unwrap_or(0);
                if fd_num == 0
                    && matches!(
                        kind,
                        ast::IoFileRedirectKind::Read | ast::IoFileRedirectKind::ReadAndWrite
                    )
                {
                    let filename = redirect_target_filename(target, state)?;
                    let path = resolve_path(&state.cwd, &filename);
                    if is_dev_stdin(&path) {
                        return Ok(default_stdin.to_string());
                    }
                    if is_dev_null(&path) || is_dev_zero(&path) || is_dev_full(&path) {
                        return Ok(String::new());
                    }
                    let content = state
                        .fs
                        .read_file(Path::new(&path))
                        .map_err(|e| RustBashError::Execution(e.to_string()))?;
                    return Ok(String::from_utf8_lossy(&content).to_string());
                }
                // Handle <&N (DuplicateInput) for reading from persistent FDs
                if fd_num == 0 && matches!(kind, ast::IoFileRedirectKind::DuplicateInput) {
                    let dup_target = redirect_target_filename(target, state)?;
                    if let Ok(source_fd) = dup_target.parse::<i32>()
                        && let Some(pfd) = state.persistent_fds.get(&source_fd)
                    {
                        match pfd {
                            PersistentFd::InputFile(path) | PersistentFd::ReadWriteFile(path) => {
                                let content = state
                                    .fs
                                    .read_file(Path::new(path))
                                    .map_err(|e| RustBashError::Execution(e.to_string()))?;
                                return Ok(String::from_utf8_lossy(&content).to_string());
                            }
                            PersistentFd::DevNull | PersistentFd::Closed => {
                                return Ok(String::new());
                            }
                            PersistentFd::OutputFile(_) => {}
                        }
                    }
                }
            }
            ast::IoRedirect::HereString(fd, word) => {
                let fd_num = fd.unwrap_or(0);
                if fd_num == 0 {
                    let val = expand_word_to_string_mut(word, state)?;
                    if val.len() > state.limits.max_heredoc_size {
                        return Err(RustBashError::LimitExceeded {
                            limit_name: "max_heredoc_size",
                            limit_value: state.limits.max_heredoc_size,
                            actual_value: val.len(),
                        });
                    }
                    return Ok(format!("{val}\n"));
                }
            }
            ast::IoRedirect::HereDocument(fd, heredoc) => {
                let fd_num = fd.unwrap_or(0);
                if fd_num == 0 {
                    let body = if heredoc.requires_expansion {
                        expand_word_to_string_mut(&heredoc.doc, state)?
                    } else {
                        heredoc.doc.value.clone()
                    };
                    if body.len() > state.limits.max_heredoc_size {
                        return Err(RustBashError::LimitExceeded {
                            limit_name: "max_heredoc_size",
                            limit_value: state.limits.max_heredoc_size,
                            actual_value: body.len(),
                        });
                    }
                    if heredoc.remove_tabs {
                        return Ok(body
                            .lines()
                            .map(|l| l.trim_start_matches('\t'))
                            .collect::<Vec<_>>()
                            .join("\n")
                            + if body.ends_with('\n') { "\n" } else { "" });
                    }
                    return Ok(body);
                }
            }
            _ => {}
        }
    }
    Ok(default_stdin.to_string())
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
    match kind {
        ast::IoFileRedirectKind::Write | ast::IoFileRedirectKind::Clobber => {
            let fd_num = fd.unwrap_or(1);
            let filename = redirect_target_filename(target, state)?;
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
            let filename = redirect_target_filename(target, state)?;
            let path = resolve_path(&state.cwd, &filename);
            apply_write_redirect(fd_num, &path, result, state, true, deferred_errors)?;
        }
        ast::IoFileRedirectKind::DuplicateOutput => {
            let fd_num = fd.unwrap_or(1);
            apply_duplicate_output(fd_num, target, result, state)?;
        }
        ast::IoFileRedirectKind::DuplicateInput => {
            // <&N — handled in get_stdin_from_redirects for FD 0
        }
        ast::IoFileRedirectKind::Read => {
            // Handled in get_stdin_from_redirects
        }
        ast::IoFileRedirectKind::ReadAndWrite => {
            let fd_num = fd.unwrap_or(0);
            let filename = redirect_target_filename(target, state)?;
            let path = resolve_path(&state.cwd, &filename);
            if !state.fs.exists(Path::new(&path)) {
                state
                    .fs
                    .write_file(Path::new(&path), b"")
                    .map_err(|e| RustBashError::Execution(e.to_string()))?;
            }
            // For FD 0, input is handled in get_stdin_from_redirects.
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
        let content = if fd_num == 1 {
            result.stdout.clone()
        } else if fd_num == 2 {
            result.stderr.clone()
        } else {
            return write_to_persistent_fd(fd_num, result, state);
        };
        write_or_append(state, path, &content, append)?;
        if fd_num == 1 {
            result.stdout.clear();
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
fn apply_duplicate_output(
    fd_num: i32,
    target: &ast::IoFileRedirectTarget,
    result: &mut ExecResult,
    state: &mut InterpreterState,
) -> Result<(), RustBashError> {
    let dup_target_str = match target {
        ast::IoFileRedirectTarget::Duplicate(word) => expand_word_to_string_mut(word, state)?,
        ast::IoFileRedirectTarget::Fd(target_fd) => target_fd.to_string(),
        _ => return Ok(()),
    };

    // Handle close: >&-
    if dup_target_str == "-" {
        if fd_num == 1 {
            result.stdout.clear();
        } else if fd_num == 2 {
            result.stderr.clear();
        }
        return Ok(());
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
        return Ok(());
    }

    // Standard duplication: N>&M
    if let Ok(target_fd) = dup_target_str.parse::<i32>() {
        apply_dup_fd(fd_num, target_fd, result, state)?;
    }
    Ok(())
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
        ast::IoFileRedirectTarget::Filename(word) => expand_word_to_string_mut(word, state),
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
        in_function_depth: 0,
        traps: HashMap::new(),
        in_trap: false,
        errexit_suppressed: 0,
        stdin_offset: 0,
        dir_stack: state.dir_stack.clone(),
        command_hash: state.command_hash.clone(),
        aliases: state.aliases.clone(),
        current_lineno: state.current_lineno,
        shell_start_time: state.shell_start_time,
        last_argument: state.last_argument.clone(),
        call_stack: state.call_stack.clone(),
        machtype: state.machtype.clone(),
        hosttype: state.hosttype.clone(),
        persistent_fds: HashMap::new(),
        next_auto_fd: 10,
        proc_sub_counter: state.proc_sub_counter,
        proc_sub_prealloc: HashMap::new(),
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
    let p = Path::new(path);

    if append {
        if state.fs.exists(p) {
            state
                .fs
                .append_file(p, content.as_bytes())
                .map_err(|e| RustBashError::Execution(e.to_string()))?;
        } else {
            state
                .fs
                .write_file(p, content.as_bytes())
                .map_err(|e| RustBashError::Execution(e.to_string()))?;
        }
    } else {
        state
            .fs
            .write_file(p, content.as_bytes())
            .map_err(|e| RustBashError::Execution(e.to_string()))?;
    }
    Ok(())
}

// ── Extended test ([[ ]]) ──────────────────────────────────────────

fn execute_extended_test(
    expr: &ast::ExtendedTestExpr,
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let result = eval_extended_test_expr(expr, state)?;
    Ok(ExecResult {
        exit_code: if result { 0 } else { 1 },
        ..ExecResult::default()
    })
}

fn eval_extended_test_expr(
    expr: &ast::ExtendedTestExpr,
    state: &mut InterpreterState,
) -> Result<bool, RustBashError> {
    match expr {
        ast::ExtendedTestExpr::And(left, right) => {
            let l = eval_extended_test_expr(left, state)?;
            let r = eval_extended_test_expr(right, state)?;
            Ok(l && r)
        }
        ast::ExtendedTestExpr::Or(left, right) => {
            let l = eval_extended_test_expr(left, state)?;
            let r = eval_extended_test_expr(right, state)?;
            Ok(l || r)
        }
        ast::ExtendedTestExpr::Not(inner) => {
            let val = eval_extended_test_expr(inner, state)?;
            Ok(!val)
        }
        ast::ExtendedTestExpr::Parenthesized(inner) => eval_extended_test_expr(inner, state),
        ast::ExtendedTestExpr::UnaryTest(pred, word) => {
            let operand = expand_word_to_string_mut(word, state)?;
            let env: HashMap<String, String> = state
                .env
                .iter()
                .map(|(k, v)| (k.clone(), v.value.as_scalar().to_string()))
                .collect();
            Ok(crate::commands::test_cmd::eval_unary_predicate(
                pred, &operand, &*state.fs, &state.cwd, &env,
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
                // For =~, expand the right side but preserve it as a regex pattern
                let pattern = expand_word_to_string_mut(right_word, state)?;
                return eval_regex_match(&left, &pattern, state);
            }

            let right = expand_word_to_string_mut(right_word, state)?;

            // Pattern matching (glob) for == and != inside [[
            if state.shopt_opts.nocasematch {
                // Case-insensitive pattern matching
                let result = match pred {
                    ast::BinaryPredicate::StringExactlyMatchesPattern => {
                        crate::interpreter::pattern::glob_match_nocase(&right, &left)
                    }
                    ast::BinaryPredicate::StringDoesNotExactlyMatchPattern => {
                        !crate::interpreter::pattern::glob_match_nocase(&right, &left)
                    }
                    ast::BinaryPredicate::StringExactlyMatchesString => {
                        left.eq_ignore_ascii_case(&right)
                    }
                    ast::BinaryPredicate::StringDoesNotExactlyMatchString => {
                        !left.eq_ignore_ascii_case(&right)
                    }
                    _ => {
                        crate::commands::test_cmd::eval_binary_predicate(pred, &left, &right, true)
                    }
                };
                Ok(result)
            } else {
                Ok(crate::commands::test_cmd::eval_binary_predicate(
                    pred, &left, &right, true,
                ))
            }
        }
    }
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
