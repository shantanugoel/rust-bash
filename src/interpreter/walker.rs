//! AST walking: execution of programs, compound lists, pipelines, and simple commands.

use crate::commands::{CommandContext, CommandResult};
use crate::error::RustBashError;
use crate::interpreter::builtins::{self, resolve_path};
use crate::interpreter::expansion::{expand_word_mut, expand_word_to_string_mut};
use crate::interpreter::{
    ExecResult, ExecutionCounters, FunctionDef, InterpreterState, Variable, execute_trap, parse,
    set_variable,
};
use crate::vfs::InMemoryFs;
use brush_parser::ast;
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
        return Err(RustBashError::LimitExceeded(format!(
            "command count ({}) exceeded limit ({})",
            state.counters.command_count, state.limits.max_command_count
        )));
    }
    if state.counters.output_size > state.limits.max_output_size {
        return Err(RustBashError::LimitExceeded(format!(
            "output size ({}) exceeded limit ({})",
            state.counters.output_size, state.limits.max_output_size
        )));
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

    // Negated pipelines (`! cmd`) suppress errexit for the inner commands
    if pipeline.bang {
        state.errexit_suppressed += 1;
    }

    for command in &pipeline.seq {
        if state.should_exit || state.control_flow.is_some() {
            break;
        }
        let r = execute_command(command, state, &pipe_data)?;
        pipe_data = r.stdout;
        combined_stderr.push_str(&r.stderr);
        exit_code = r.exit_code;
        exit_codes.push(r.exit_code);
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
    match command {
        ast::Command::Simple(simple_cmd) => execute_simple_command(simple_cmd, state, stdin),
        ast::Command::Compound(compound, redirects) => {
            execute_compound_command(compound, redirects.as_ref(), state, stdin)
        }
        ast::Command::Function(func_def) => {
            let name = expand_word_to_string_mut(&func_def.fname, state)?;
            state.functions.insert(
                name,
                FunctionDef {
                    body: func_def.body.clone(),
                },
            );
            Ok(ExecResult::default())
        }
        ast::Command::ExtendedTest(ext_test) => execute_extended_test(&ext_test.expr, state),
    }
}

fn execute_simple_command(
    cmd: &ast::SimpleCommand,
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    // 1. Collect redirections and assignments from prefix
    let mut assignments: Vec<(String, String)> = Vec::new();
    let mut redirects: Vec<&ast::IoRedirect> = Vec::new();

    if let Some(prefix) = &cmd.prefix {
        for item in &prefix.0 {
            match item {
                ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, _) => {
                    let name = match &assignment.name {
                        ast::AssignmentName::VariableName(n) => n.clone(),
                        ast::AssignmentName::ArrayElementName(n, _) => n.clone(),
                    };
                    let value = match &assignment.value {
                        ast::AssignmentValue::Scalar(w) => expand_word_to_string_mut(w, state)?,
                        ast::AssignmentValue::Array(_) => String::new(),
                    };
                    assignments.push((name, value));
                }
                ast::CommandPrefixOrSuffixItem::IoRedirect(redir) => {
                    redirects.push(redir);
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
                ast::CommandPrefixOrSuffixItem::Word(w) => {
                    let expanded = expand_word_mut(w, state)?;
                    args.extend(expanded);
                }
                ast::CommandPrefixOrSuffixItem::IoRedirect(redir) => {
                    redirects.push(redir);
                }
                ast::CommandPrefixOrSuffixItem::AssignmentWord(assignment, _) => {
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
                _ => {}
            }
        }
    }

    // 4. No command name → persist assignments in environment
    let Some(cmd_name) = cmd_name else {
        for (name, value) in assignments {
            set_variable(state, &name, value)?;
        }
        return Ok(ExecResult::default());
    };

    // 4b. Empty command name (e.g. from `$(false)`) → no command, persist assignments
    if cmd_name.is_empty() && args.is_empty() {
        for (name, value) in assignments {
            set_variable(state, &name, value)?;
        }
        return Ok(ExecResult {
            exit_code: state.last_exit_code,
            ..ExecResult::default()
        });
    }

    // 5. Apply temporary pre-command assignments
    let mut saved: Vec<(String, Option<Variable>)> = Vec::new();
    for (name, value) in &assignments {
        saved.push((name.clone(), state.env.get(name).cloned()));
        set_variable(state, name, value.clone())?;
    }

    // 6. Handle stdin redirection
    let effective_stdin = get_stdin_from_redirects(&redirects, state, stdin)?;

    // 7. Dispatch command
    let mut result = dispatch_command(&cmd_name, &args, state, &effective_stdin)?;

    // 8. Apply output redirections
    apply_output_redirects(&redirects, &mut result, state)?;

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

    Ok(result)
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
            return Err(RustBashError::Execution(format!(
                "for loop exceeded maximum iterations ({})",
                state.limits.max_loop_iterations
            )));
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
            return Err(RustBashError::Execution(format!(
                "arithmetic for loop exceeded maximum iterations ({})",
                state.limits.max_loop_iterations
            )));
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
            return Err(RustBashError::Execution(format!(
                "loop exceeded maximum iterations ({})",
                state.limits.max_loop_iterations
            )));
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
    let original_fs = &state.fs;
    let cloned_fs: Arc<dyn crate::vfs::VirtualFs> =
        if let Some(memfs) = original_fs.as_any().downcast_ref::<InMemoryFs>() {
            Arc::new(memfs.deep_clone())
        } else {
            Arc::clone(original_fs)
        };

    let mut sub_state = InterpreterState {
        fs: cloned_fs,
        env: state.env.clone(),
        cwd: state.cwd.clone(),
        functions: state.functions.clone(),
        last_exit_code: state.last_exit_code,
        commands: clone_commands(&state.commands),
        shell_opts: state.shell_opts.clone(),
        limits: state.limits.clone(),
        counters: ExecutionCounters::default(),
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
    };

    let result = execute_compound_list(list, &mut sub_state, stdin)?;

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
                if crate::interpreter::pattern::glob_match(&pattern, &value) {
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
fn make_exec_callback(
    state: &InterpreterState,
) -> impl Fn(&str) -> Result<CommandResult, RustBashError> {
    let cloned_fs: Arc<dyn crate::vfs::VirtualFs> =
        if let Some(memfs) = state.fs.as_any().downcast_ref::<InMemoryFs>() {
            Arc::new(memfs.deep_clone())
        } else {
            Arc::clone(&state.fs)
        };
    let env = state.env.clone();
    let cwd = state.cwd.clone();
    let functions = state.functions.clone();
    let last_exit_code = state.last_exit_code;
    let commands = clone_commands(&state.commands);
    let shell_opts = state.shell_opts.clone();
    let limits = state.limits.clone();
    let positional_params = state.positional_params.clone();
    let shell_name = state.shell_name.clone();
    let random_seed = state.random_seed;

    move |cmd_str: &str| {
        let program = parse(cmd_str)?;

        let sub_fs: Arc<dyn crate::vfs::VirtualFs> =
            if let Some(memfs) = cloned_fs.as_any().downcast_ref::<InMemoryFs>() {
                Arc::new(memfs.deep_clone())
            } else {
                Arc::clone(&cloned_fs)
            };

        let mut sub_state = InterpreterState {
            fs: sub_fs,
            env: env.clone(),
            cwd: cwd.clone(),
            functions: functions.clone(),
            last_exit_code,
            commands: clone_commands(&commands),
            shell_opts: shell_opts.clone(),
            limits: limits.clone(),
            counters: ExecutionCounters::default(),
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
        state.counters.call_depth -= 1;
        return Err(RustBashError::Execution(format!(
            "{name}: maximum function call depth exceeded ({})",
            state.limits.max_call_depth
        )));
    }

    // Clone the function body so we don't hold a borrow on state.functions
    let func_def = state.functions.get(name).unwrap().clone();

    // Save and replace positional parameters
    let saved_params = std::mem::replace(&mut state.positional_params, args.to_vec());

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
            .map(|(k, v)| (k.clone(), v.value.clone()))
            .collect();
        let fs = Arc::clone(&state.fs);
        let cwd = state.cwd.clone();
        let limits = state.limits.clone();

        let exec_callback = make_exec_callback(state);

        let ctx = CommandContext {
            fs: &*fs,
            cwd: &cwd,
            env: &env,
            stdin,
            limits: &limits,
            exec: Some(&exec_callback),
        };

        let cmd_result = cmd.execute(args, &ctx);
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

// ── Redirections ────────────────────────────────────────────────────

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
                    && let ast::IoFileRedirectKind::Read = kind
                {
                    let filename = redirect_target_filename(target, state)?;
                    let path = resolve_path(&state.cwd, &filename);
                    let content = state
                        .fs
                        .read_file(Path::new(&path))
                        .map_err(|e| RustBashError::Execution(e.to_string()))?;
                    return Ok(String::from_utf8_lossy(&content).to_string());
                }
            }
            ast::IoRedirect::HereString(fd, word) => {
                let fd_num = fd.unwrap_or(0);
                if fd_num == 0 {
                    let val = expand_word_to_string_mut(word, state)?;
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
    for redir in redirects {
        match redir {
            ast::IoRedirect::File(fd, kind, target) => {
                apply_file_redirect(*fd, kind, target, result, state)?;
            }
            ast::IoRedirect::OutputAndError(word, append) => {
                let filename = expand_word_to_string_mut(word, state)?;
                let path = resolve_path(&state.cwd, &filename);
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
    Ok(())
}

fn apply_file_redirect(
    fd: Option<i32>,
    kind: &ast::IoFileRedirectKind,
    target: &ast::IoFileRedirectTarget,
    result: &mut ExecResult,
    state: &mut InterpreterState,
) -> Result<(), RustBashError> {
    match kind {
        ast::IoFileRedirectKind::Write | ast::IoFileRedirectKind::Clobber => {
            let fd_num = fd.unwrap_or(1);
            let filename = redirect_target_filename(target, state)?;
            let path = resolve_path(&state.cwd, &filename);

            if is_dev_null(&path) {
                if fd_num == 1 {
                    result.stdout.clear();
                } else if fd_num == 2 {
                    result.stderr.clear();
                }
            } else {
                let content = if fd_num == 1 {
                    result.stdout.clone()
                } else if fd_num == 2 {
                    result.stderr.clone()
                } else {
                    return Ok(());
                };
                write_or_append(state, &path, &content, false)?;
                if fd_num == 1 {
                    result.stdout.clear();
                } else if fd_num == 2 {
                    result.stderr.clear();
                }
            }
        }
        ast::IoFileRedirectKind::Append => {
            let fd_num = fd.unwrap_or(1);
            let filename = redirect_target_filename(target, state)?;
            let path = resolve_path(&state.cwd, &filename);

            if is_dev_null(&path) {
                if fd_num == 1 {
                    result.stdout.clear();
                } else if fd_num == 2 {
                    result.stderr.clear();
                }
            } else {
                let content = if fd_num == 1 {
                    result.stdout.clone()
                } else if fd_num == 2 {
                    result.stderr.clone()
                } else {
                    return Ok(());
                };
                write_or_append(state, &path, &content, true)?;
                if fd_num == 1 {
                    result.stdout.clear();
                } else if fd_num == 2 {
                    result.stderr.clear();
                }
            }
        }
        ast::IoFileRedirectKind::DuplicateOutput => {
            let fd_num = fd.unwrap_or(1);
            // Handle >&1, >&2, 2>&1, etc.
            if let ast::IoFileRedirectTarget::Duplicate(word) = target {
                let dup_target = expand_word_to_string_mut(word, state)?;
                if dup_target == "1" && fd_num == 2 {
                    // 2>&1: merge stderr into stdout
                    result.stdout.push_str(&result.stderr);
                    result.stderr.clear();
                } else if dup_target == "2" && fd_num == 1 {
                    // 1>&2: merge stdout into stderr
                    result.stderr.push_str(&result.stdout);
                    result.stdout.clear();
                }
            } else if let ast::IoFileRedirectTarget::Fd(target_fd) = target {
                if *target_fd == 1 && fd_num == 2 {
                    result.stdout.push_str(&result.stderr);
                    result.stderr.clear();
                } else if *target_fd == 2 && fd_num == 1 {
                    result.stderr.push_str(&result.stdout);
                    result.stdout.clear();
                }
            }
        }
        ast::IoFileRedirectKind::Read => {
            // Handled in get_stdin_from_redirects
        }
        ast::IoFileRedirectKind::ReadAndWrite | ast::IoFileRedirectKind::DuplicateInput => {
            // Not commonly used; skip for now
        }
    }
    Ok(())
}

fn redirect_target_filename(
    target: &ast::IoFileRedirectTarget,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    match target {
        ast::IoFileRedirectTarget::Filename(word) => expand_word_to_string_mut(word, state),
        ast::IoFileRedirectTarget::Fd(fd) => Ok(fd.to_string()),
        ast::IoFileRedirectTarget::Duplicate(word) => expand_word_to_string_mut(word, state),
        ast::IoFileRedirectTarget::ProcessSubstitution(_, _) => Err(RustBashError::Execution(
            "process substitution not yet implemented".into(),
        )),
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
                .map(|(k, v)| (k.clone(), v.value.clone()))
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
            Ok(crate::commands::test_cmd::eval_binary_predicate(
                pred, &left, &right, true,
            ))
        }
    }
}

fn eval_regex_match(
    string: &str,
    pattern: &str,
    state: &mut InterpreterState,
) -> Result<bool, RustBashError> {
    let re = regex::Regex::new(pattern)
        .map_err(|e| RustBashError::Execution(format!("invalid regex '{pattern}': {e}")))?;

    if let Some(captures) = re.captures(string) {
        // Store BASH_REMATCH[0] = whole match
        let whole = captures.get(0).map(|m| m.as_str()).unwrap_or("");
        set_variable(state, "BASH_REMATCH", whole.to_string())?;

        // Store capture groups as BASH_REMATCH_1, BASH_REMATCH_2, etc.
        // Also store count for potential array-like access
        for i in 1..captures.len() {
            let val = captures.get(i).map(|m| m.as_str()).unwrap_or("");
            set_variable(state, &format!("BASH_REMATCH_{i}"), val.to_string())?;
        }
        set_variable(state, "BASH_REMATCH_COUNT", captures.len().to_string())?;

        Ok(true)
    } else {
        // Clear BASH_REMATCH on non-match
        set_variable(state, "BASH_REMATCH", String::new())?;
        set_variable(state, "BASH_REMATCH_COUNT", "0".to_string())?;
        Ok(false)
    }
}
