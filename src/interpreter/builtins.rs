//! Shell builtins that modify interpreter state.

use crate::error::RustBashError;
use crate::interpreter::walker::execute_program;
use crate::interpreter::{
    ControlFlow, ExecResult, InterpreterState, Variable, parse, set_variable,
};
use crate::vfs::NodeType;
use std::path::Path;

/// Dispatch a shell builtin by name.
/// Returns `Ok(Some(result))` if the name is a recognised builtin,
/// `Ok(None)` if not.
pub(crate) fn execute_builtin(
    name: &str,
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<Option<ExecResult>, RustBashError> {
    match name {
        "exit" => builtin_exit(args, state).map(Some),
        "cd" => builtin_cd(args, state).map(Some),
        "export" => builtin_export(args, state).map(Some),
        "unset" => builtin_unset(args, state).map(Some),
        "set" => builtin_set(args, state).map(Some),
        "shift" => builtin_shift(args, state).map(Some),
        "readonly" => builtin_readonly(args, state).map(Some),
        "declare" => builtin_declare(args, state).map(Some),
        "read" => builtin_read(args, state, stdin).map(Some),
        "eval" => builtin_eval(args, state).map(Some),
        "source" | "." => builtin_source(args, state).map(Some),
        "break" => builtin_break(args, state).map(Some),
        "continue" => builtin_continue(args, state).map(Some),
        ":" | "colon" => Ok(Some(ExecResult::default())),
        "let" => builtin_let(args, state).map(Some),
        "local" => builtin_local(args, state).map(Some),
        "return" => builtin_return(args, state).map(Some),
        "trap" => builtin_trap(args, state).map(Some),
        _ => Ok(None),
    }
}

// ── exit ─────────────────────────────────────────────────────────────

fn builtin_exit(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    state.should_exit = true;
    let code = if let Some(arg) = args.first() {
        match arg.parse::<i32>() {
            Ok(n) => n,
            Err(_) => {
                return Ok(ExecResult {
                    stderr: format!("exit: {arg}: numeric argument required\n"),
                    exit_code: 2,
                    ..ExecResult::default()
                });
            }
        }
    } else {
        state.last_exit_code
    };
    Ok(ExecResult {
        exit_code: code & 0xFF,
        ..ExecResult::default()
    })
}

// ── break ────────────────────────────────────────────────────────────

fn builtin_break(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let n = parse_loop_level("break", args)?;
    let n = match n {
        Ok(level) => level,
        Err(result) => return Ok(result),
    };
    if state.loop_depth == 0 {
        return Ok(ExecResult {
            stderr: "break: only meaningful in a `for', `while', or `until' loop\n".to_string(),
            exit_code: 1,
            ..ExecResult::default()
        });
    }
    state.control_flow = Some(ControlFlow::Break(n.min(state.loop_depth)));
    Ok(ExecResult::default())
}

// ── continue ─────────────────────────────────────────────────────────

fn builtin_continue(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let n = parse_loop_level("continue", args)?;
    let n = match n {
        Ok(level) => level,
        Err(result) => return Ok(result),
    };
    if state.loop_depth == 0 {
        return Ok(ExecResult {
            stderr: "continue: only meaningful in a `for', `while', or `until' loop\n".to_string(),
            exit_code: 1,
            ..ExecResult::default()
        });
    }
    state.control_flow = Some(ControlFlow::Continue(n.min(state.loop_depth)));
    Ok(ExecResult::default())
}

/// Parse the optional numeric level argument for break/continue.
/// Returns `Ok(Ok(n))` on success, `Ok(Err(result))` for user-facing errors.
fn parse_loop_level(
    name: &str,
    args: &[String],
) -> Result<Result<usize, ExecResult>, RustBashError> {
    if let Some(arg) = args.first() {
        match arg.parse::<isize>() {
            Ok(n) if n <= 0 => Ok(Err(ExecResult {
                stderr: format!("{name}: {arg}: loop count out of range\n"),
                exit_code: 1,
                ..ExecResult::default()
            })),
            Ok(n) => Ok(Ok(n as usize)),
            Err(_) => Ok(Err(ExecResult {
                stderr: format!("{name}: {arg}: numeric argument required\n"),
                exit_code: 128,
                ..ExecResult::default()
            })),
        }
    } else {
        Ok(Ok(1))
    }
}

// ── cd ──────────────────────────────────────────────────────────────

fn builtin_cd(args: &[String], state: &mut InterpreterState) -> Result<ExecResult, RustBashError> {
    let target = if args.is_empty() {
        // cd with no args → $HOME
        match state.env.get("HOME") {
            Some(v) if !v.value.is_empty() => v.value.clone(),
            _ => {
                return Ok(ExecResult {
                    stderr: "cd: HOME not set\n".to_string(),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
        }
    } else if args[0] == "-" {
        // cd - → $OLDPWD
        match state.env.get("OLDPWD") {
            Some(v) if !v.value.is_empty() => v.value.clone(),
            _ => {
                return Ok(ExecResult {
                    stderr: "cd: OLDPWD not set\n".to_string(),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
        }
    } else {
        args[0].clone()
    };

    // Resolve path (relative to cwd)
    let resolved = resolve_path(&state.cwd, &target);

    // Validate the path exists and is a directory
    let path = Path::new(&resolved);
    if !state.fs.exists(path) {
        return Ok(ExecResult {
            stderr: format!("cd: {target}: No such file or directory\n"),
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    match state.fs.stat(path) {
        Ok(meta) if meta.node_type == NodeType::Directory => {}
        _ => {
            return Ok(ExecResult {
                stderr: format!("cd: {target}: Not a directory\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
        }
    }

    let old_cwd = state.cwd.clone();
    state.cwd = resolved;

    // Set OLDPWD — use set_variable to respect readonly
    let _ = set_variable(state, "OLDPWD", old_cwd);
    if let Some(var) = state.env.get_mut("OLDPWD") {
        var.exported = true;
    }
    let new_cwd = state.cwd.clone();
    let _ = set_variable(state, "PWD", new_cwd);
    if let Some(var) = state.env.get_mut("PWD") {
        var.exported = true;
    }

    // If cd -, print the new directory
    let stdout = if !args.is_empty() && args[0] == "-" {
        format!("{}\n", state.cwd)
    } else {
        String::new()
    };

    Ok(ExecResult {
        stdout,
        ..ExecResult::default()
    })
}

/// Resolve a potentially relative path against a base directory.
pub(crate) fn resolve_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        normalize_path(path)
    } else {
        let combined = if cwd.ends_with('/') {
            format!("{cwd}{path}")
        } else {
            format!("{cwd}/{path}")
        };
        normalize_path(&combined)
    }
}

fn normalize_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

// ── export ──────────────────────────────────────────────────────────

fn builtin_export(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // List all exported variables
        let mut lines: Vec<String> = state
            .env
            .iter()
            .filter(|(_, v)| v.exported)
            .map(|(k, v)| format!("declare -x {k}=\"{}\"\n", v.value))
            .collect();
        lines.sort();
        return Ok(ExecResult {
            stdout: lines.join(""),
            ..ExecResult::default()
        });
    }

    for arg in args {
        if arg == "-n" {
            continue; // export -n VAR would unexport, skip flag for now
        }
        if arg.starts_with('-') && !arg.contains('=') {
            continue; // skip other flags
        }
        if let Some((name, value)) = arg.split_once('=') {
            set_variable(state, name, value.to_string())?;
            if let Some(var) = state.env.get_mut(name) {
                var.exported = true;
            }
        } else {
            // Just mark existing variable as exported
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                var.exported = true;
            } else {
                // Create empty exported variable
                state.env.insert(
                    arg.clone(),
                    Variable {
                        value: String::new(),
                        exported: true,
                        readonly: false,
                    },
                );
            }
        }
    }

    Ok(ExecResult::default())
}

// ── unset ───────────────────────────────────────────────────────────

fn builtin_unset(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    for arg in args {
        if arg.starts_with('-') {
            continue; // skip flags like -v, -f
        }
        if let Some(var) = state.env.get(arg.as_str())
            && var.readonly
        {
            return Ok(ExecResult {
                stderr: format!("unset: {arg}: cannot unset: readonly variable\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
        }
        state.env.remove(arg.as_str());
    }
    Ok(ExecResult::default())
}

// ── set ─────────────────────────────────────────────────────────────

fn builtin_set(args: &[String], state: &mut InterpreterState) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // List all variables
        let mut lines: Vec<String> = state
            .env
            .iter()
            .map(|(k, v)| format!("{k}='{}'\n", v.value))
            .collect();
        lines.sort();
        return Ok(ExecResult {
            stdout: lines.join(""),
            ..ExecResult::default()
        });
    }

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            // Everything after -- becomes positional parameters
            state.positional_params = args[i + 1..].to_vec();
            return Ok(ExecResult::default());
        } else if arg.starts_with('+') || arg.starts_with('-') {
            let enable = arg.starts_with('-');
            if arg == "-o" || arg == "+o" {
                i += 1;
                if i < args.len() {
                    apply_option_name(&args[i], enable, state);
                } else if !enable {
                    // set +o with no arg: list options
                    return Ok(ExecResult {
                        stdout: format_options(state),
                        ..ExecResult::default()
                    });
                } else {
                    return Ok(ExecResult {
                        stdout: format_options(state),
                        ..ExecResult::default()
                    });
                }
            } else {
                let chars: Vec<char> = arg[1..].chars().collect();
                let mut saw_o = false;
                for c in &chars {
                    if *c == 'o' {
                        saw_o = true;
                    } else {
                        apply_option_char(*c, enable, state);
                    }
                }
                if saw_o {
                    // 'o' in a flag group (e.g., -eo) consumes next arg as option name
                    i += 1;
                    if i < args.len() {
                        apply_option_name(&args[i], enable, state);
                    }
                }
            }
        } else {
            // Positional parameters
            state.positional_params = args[i..].to_vec();
            return Ok(ExecResult::default());
        }
        i += 1;
    }

    Ok(ExecResult::default())
}

fn apply_option_char(c: char, enable: bool, state: &mut InterpreterState) {
    match c {
        'e' => state.shell_opts.errexit = enable,
        'u' => state.shell_opts.nounset = enable,
        'x' => state.shell_opts.xtrace = enable,
        _ => {}
    }
}

fn apply_option_name(name: &str, enable: bool, state: &mut InterpreterState) {
    match name {
        "errexit" => state.shell_opts.errexit = enable,
        "nounset" => state.shell_opts.nounset = enable,
        "pipefail" => state.shell_opts.pipefail = enable,
        "xtrace" => state.shell_opts.xtrace = enable,
        _ => {}
    }
}

fn format_options(state: &InterpreterState) -> String {
    let on_off = |b: bool| if b { "on" } else { "off" };
    format!(
        "errexit        {}\nnounset        {}\npipefail       {}\nxtrace         {}\n",
        on_off(state.shell_opts.errexit),
        on_off(state.shell_opts.nounset),
        on_off(state.shell_opts.pipefail),
        on_off(state.shell_opts.xtrace),
    )
}

// ── shift ───────────────────────────────────────────────────────────

fn builtin_shift(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let n = if let Some(arg) = args.first() {
        match arg.parse::<usize>() {
            Ok(n) => n,
            Err(_) => {
                return Ok(ExecResult {
                    stderr: format!("shift: {arg}: numeric argument required\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
        }
    } else {
        1
    };

    if n > state.positional_params.len() {
        return Ok(ExecResult {
            stderr: format!("shift: {n}: shift count out of range\n"),
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    state.positional_params = state.positional_params[n..].to_vec();
    Ok(ExecResult::default())
}

// ── readonly ────────────────────────────────────────────────────────

fn builtin_readonly(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        let mut lines: Vec<String> = state
            .env
            .iter()
            .filter(|(_, v)| v.readonly)
            .map(|(k, v)| format!("declare -r {k}=\"{}\"\n", v.value))
            .collect();
        lines.sort();
        return Ok(ExecResult {
            stdout: lines.join(""),
            ..ExecResult::default()
        });
    }

    for arg in args {
        if arg.starts_with('-') {
            continue; // skip flags
        }
        if let Some((name, value)) = arg.split_once('=') {
            set_variable(state, name, value.to_string())?;
            if let Some(var) = state.env.get_mut(name) {
                var.readonly = true;
            }
        } else {
            // Mark existing variable as readonly
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                var.readonly = true;
            } else {
                state.env.insert(
                    arg.clone(),
                    Variable {
                        value: String::new(),
                        exported: false,
                        readonly: true,
                    },
                );
            }
        }
    }

    Ok(ExecResult::default())
}

// ── declare ─────────────────────────────────────────────────────────

fn builtin_declare(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let mut make_readonly = false;
    let mut make_exported = false;
    let mut var_args: Vec<&String> = Vec::new();

    for arg in args {
        if let Some(flags) = arg.strip_prefix('-') {
            for c in flags.chars() {
                match c {
                    'r' => make_readonly = true,
                    'x' => make_exported = true,
                    _ => {}
                }
            }
        } else {
            var_args.push(arg);
        }
    }

    if var_args.is_empty() && !make_readonly && !make_exported {
        // declare with no args — list all variables
        let mut lines: Vec<String> = state
            .env
            .iter()
            .map(|(k, v)| {
                let mut attrs = String::from("declare ");
                if v.readonly {
                    attrs.push('r');
                }
                if v.exported {
                    attrs.push('x');
                }
                if !v.readonly && !v.exported {
                    attrs.push('-');
                    attrs.push('-');
                }
                format!("{attrs} {k}=\"{}\"\n", v.value)
            })
            .collect();
        lines.sort();
        return Ok(ExecResult {
            stdout: lines.join(""),
            ..ExecResult::default()
        });
    }

    for arg in var_args {
        if let Some((name, value)) = arg.split_once('=') {
            set_variable(state, name, value.to_string())?;
            if let Some(var) = state.env.get_mut(name) {
                if make_readonly {
                    var.readonly = true;
                }
                if make_exported {
                    var.exported = true;
                }
            }
        } else {
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                if make_readonly {
                    var.readonly = true;
                }
                if make_exported {
                    var.exported = true;
                }
            } else {
                state.env.insert(
                    arg.clone(),
                    Variable {
                        value: String::new(),
                        exported: make_exported,
                        readonly: make_readonly,
                    },
                );
            }
        }
    }

    Ok(ExecResult::default())
}

// ── read ────────────────────────────────────────────────────────────

fn builtin_read(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let mut raw_mode = false;
    let mut var_names: Vec<&str> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg == "-r" {
            raw_mode = true;
        } else if arg == "-p" {
            // Prompt — no-op in sandbox, skip the following prompt string
            i += 1;
        } else if arg.starts_with('-') {
            // Skip other flags
        } else {
            var_names.push(arg);
        }
        i += 1;
    }

    if var_names.is_empty() {
        var_names.push("REPLY");
    }

    // Read one line from stdin, starting at the current offset
    let effective_stdin = if state.stdin_offset < stdin.len() {
        &stdin[state.stdin_offset..]
    } else {
        ""
    };
    let line = match effective_stdin.lines().next() {
        Some(l) => {
            // Advance offset past this line and its newline
            state.stdin_offset += l.len();
            if state.stdin_offset < stdin.len()
                && stdin.as_bytes().get(state.stdin_offset) == Some(&b'\n')
            {
                state.stdin_offset += 1;
            }
            l
        }
        None => {
            // EOF — no more input
            return Ok(ExecResult {
                exit_code: 1,
                ..ExecResult::default()
            });
        }
    };

    // Check input line length before processing
    if line.len() > state.limits.max_string_length {
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_string_length",
            limit_value: state.limits.max_string_length,
            actual_value: line.len(),
        });
    }

    let line = if raw_mode {
        line.to_string()
    } else {
        // Process backslash escapes: backslash-newline is line continuation (remove both)
        let mut result = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(&next) = chars.peek() {
                    if next == '\n' {
                        chars.next(); // skip newline
                    } else {
                        result.push(next);
                        chars.next();
                    }
                }
            } else {
                result.push(c);
            }
        }
        result
    };

    let ifs = state
        .env
        .get("IFS")
        .map(|v| v.value.clone())
        .unwrap_or_else(|| " \t\n".to_string());

    let fields: Vec<&str> = if ifs.is_empty() {
        vec![line.as_str()]
    } else {
        split_by_ifs(&line, &ifs)
    };

    // Assign fields to variables
    for (i, var_name) in var_names.iter().enumerate() {
        let value = if i == var_names.len() - 1 && fields.len() > var_names.len() {
            // Last variable gets the remaining fields
            let remaining: Vec<&str> = fields[i..].to_vec();
            remaining.join(&ifs[..1.min(ifs.len())])
        } else {
            fields.get(i).unwrap_or(&"").to_string()
        };
        set_variable(state, var_name, value)?;
    }

    // Return 1 if stdin was empty (EOF)
    let exit_code = if stdin.is_empty() { 1 } else { 0 };

    Ok(ExecResult {
        exit_code,
        ..ExecResult::default()
    })
}

fn split_by_ifs<'a>(s: &'a str, ifs: &str) -> Vec<&'a str> {
    let has_whitespace = ifs.contains(' ') || ifs.contains('\t') || ifs.contains('\n');

    if has_whitespace {
        // IFS whitespace splitting: leading/trailing whitespace is trimmed,
        // consecutive whitespace chars are treated as one delimiter
        s.split(|c: char| ifs.contains(c))
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        // Non-whitespace IFS: split on each char, preserve empty fields
        s.split(|c: char| ifs.contains(c)).collect()
    }
}

// ── eval ─────────────────────────────────────────────────────────────

fn builtin_eval(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        return Ok(ExecResult::default());
    }

    let input = args.join(" ");
    if input.is_empty() {
        return Ok(ExecResult::default());
    }

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

    let program = match parse(&input) {
        Ok(p) => p,
        Err(e) => {
            state.counters.call_depth -= 1;
            return Err(e);
        }
    };
    let result = execute_program(&program, state);
    state.counters.call_depth -= 1;
    result
}

/// Common signal names for `trap -l`.
const SIGNAL_NAMES: &[&str] = &[
    "EXIT", "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV",
    "USR2", "PIPE", "ALRM", "TERM", "STKFLT", "CHLD", "CONT", "STOP", "TSTP", "TTIN", "TTOU",
    "URG", "XCPU", "XFSZ", "VTALRM", "PROF", "WINCH", "IO", "PWR", "SYS", "ERR", "DEBUG", "RETURN",
];

/// Normalize signal name: strip leading "SIG" prefix and uppercase.
fn normalize_signal(name: &str) -> String {
    let upper = name.to_uppercase();
    upper.strip_prefix("SIG").unwrap_or(&upper).to_string()
}

fn builtin_trap(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    // `trap` with no args — list current traps
    if args.is_empty() {
        let mut out = String::new();
        let mut names: Vec<&String> = state.traps.keys().collect();
        names.sort();
        for name in names {
            let cmd = &state.traps[name];
            out.push_str(&format!(
                "trap -- '{}' {}\n",
                cmd.replace('\'', "'\\''"),
                name
            ));
        }
        return Ok(ExecResult {
            stdout: out,
            ..ExecResult::default()
        });
    }

    // `trap -l` — list signal names
    if args.len() == 1 && args[0] == "-l" {
        let out: String = SIGNAL_NAMES
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if i > 0 && i % 8 == 0 {
                    format!("\n{:2}) SIG{}", i, s)
                } else {
                    format!("{:>3}) SIG{}", i, s)
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        return Ok(ExecResult {
            stdout: format!("{out}\n"),
            ..ExecResult::default()
        });
    }

    // `trap - SIGNAL ...` — reset signals to default (remove handler)
    if args.first().map(|s| s.as_str()) == Some("-") {
        for sig in &args[1..] {
            state.traps.remove(&normalize_signal(sig));
        }
        return Ok(ExecResult::default());
    }

    // `trap 'command' SIGNAL [SIGNAL ...]`
    if args.len() < 2 {
        return Ok(ExecResult {
            stderr: "trap: usage: trap [-lp] [[arg] signal_spec ...]\n".to_string(),
            exit_code: 2,
            ..ExecResult::default()
        });
    }

    let command = &args[0];
    for sig in &args[1..] {
        let name = normalize_signal(sig);
        if command.is_empty() {
            // `trap '' SIGNAL` — register empty handler (ignore signal)
            state.traps.insert(name, String::new());
        } else {
            state.traps.insert(name, command.clone());
        }
    }

    Ok(ExecResult::default())
}

// ── source / . ──────────────────────────────────────────────────────

fn builtin_source(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let path_arg = match args.first() {
        Some(p) => p,
        None => {
            return Ok(ExecResult {
                stderr: "source: filename argument required\n".to_string(),
                exit_code: 2,
                ..ExecResult::default()
            });
        }
    };

    let resolved = resolve_path(&state.cwd, path_arg);
    let content = match state.fs.read_file(Path::new(&resolved)) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => {
            return Ok(ExecResult {
                stderr: format!("source: {path_arg}: No such file or directory\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
        }
    };

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

    let program = match parse(&content) {
        Ok(p) => p,
        Err(e) => {
            state.counters.call_depth -= 1;
            return Err(e);
        }
    };
    let result = execute_program(&program, state);
    state.counters.call_depth -= 1;
    result
}

// ── local ────────────────────────────────────────────────────────────

fn builtin_local(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    // bash allows `local` at top level (it just acts like a normal assignment)
    // but we only do scope tracking inside functions
    for arg in args {
        if arg.starts_with('-') {
            continue; // skip flags
        }
        if let Some((name, value)) = arg.split_once('=') {
            // Save current value in the top local scope (if inside a function)
            if let Some(scope) = state.local_scopes.last_mut() {
                scope
                    .entry(name.to_string())
                    .or_insert_with(|| state.env.get(name).cloned());
            }
            set_variable(state, name, value.to_string())?;
        } else {
            // `local VAR` with no value — declare it as local with empty value
            if let Some(scope) = state.local_scopes.last_mut() {
                scope
                    .entry(arg.clone())
                    .or_insert_with(|| state.env.get(arg.as_str()).cloned());
            }
            // Inside a function: always set to empty. Outside: only if undefined.
            if state.in_function_depth > 0 || !state.env.contains_key(arg.as_str()) {
                state.env.insert(
                    arg.clone(),
                    Variable {
                        value: String::new(),
                        exported: false,
                        readonly: false,
                    },
                );
            }
        }
    }
    Ok(ExecResult::default())
}

// ── return ───────────────────────────────────────────────────────────

fn builtin_return(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if state.in_function_depth == 0 {
        return Ok(ExecResult {
            stderr: "return: can only `return' from a function or sourced script\n".to_string(),
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    let code = if let Some(arg) = args.first() {
        match arg.parse::<i32>() {
            Ok(n) => n & 0xFF,
            Err(_) => {
                return Ok(ExecResult {
                    stderr: format!("return: {arg}: numeric argument required\n"),
                    exit_code: 2,
                    ..ExecResult::default()
                });
            }
        }
    } else {
        state.last_exit_code
    };

    state.control_flow = Some(ControlFlow::Return(code));
    Ok(ExecResult {
        exit_code: code,
        ..ExecResult::default()
    })
}

// ── let ─────────────────────────────────────────────────────────────

fn builtin_let(args: &[String], state: &mut InterpreterState) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        return Err(RustBashError::Execution(
            "let: usage: let arg [arg ...]".into(),
        ));
    }
    let mut last_val: i64 = 0;
    for arg in args {
        last_val = crate::interpreter::arithmetic::eval_arithmetic(arg, state)?;
    }
    // Exit code 0 if last result non-zero, 1 if zero
    Ok(ExecResult {
        exit_code: if last_val != 0 { 0 } else { 1 },
        ..ExecResult::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::{ExecutionCounters, ExecutionLimits, ShellOpts};
    use crate::network::NetworkPolicy;
    use crate::vfs::{InMemoryFs, VirtualFs};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_state() -> InterpreterState {
        let fs = Arc::new(InMemoryFs::new());
        fs.mkdir_p(Path::new("/home/user")).unwrap();

        InterpreterState {
            fs,
            env: HashMap::new(),
            cwd: "/".to_string(),
            functions: HashMap::new(),
            last_exit_code: 0,
            commands: HashMap::new(),
            shell_opts: ShellOpts::default(),
            limits: ExecutionLimits::default(),
            counters: ExecutionCounters::default(),
            network_policy: NetworkPolicy::default(),
            should_exit: false,
            loop_depth: 0,
            control_flow: None,
            positional_params: Vec::new(),
            shell_name: "rust-bash".to_string(),
            random_seed: 42,
            local_scopes: Vec::new(),
            in_function_depth: 0,
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
            stdin_offset: 0,
        }
    }

    #[test]
    fn cd_to_directory() {
        let mut state = make_state();
        let result = builtin_cd(&["/home/user".to_string()], &mut state).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(state.cwd, "/home/user");
    }

    #[test]
    fn cd_nonexistent() {
        let mut state = make_state();
        let result = builtin_cd(&["/nonexistent".to_string()], &mut state).unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("No such file or directory"));
    }

    #[test]
    fn cd_home() {
        let mut state = make_state();
        state.env.insert(
            "HOME".to_string(),
            Variable {
                value: "/home/user".to_string(),
                exported: true,
                readonly: false,
            },
        );
        let result = builtin_cd(&[], &mut state).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(state.cwd, "/home/user");
    }

    #[test]
    fn cd_dash() {
        let mut state = make_state();
        state.env.insert(
            "OLDPWD".to_string(),
            Variable {
                value: "/home/user".to_string(),
                exported: true,
                readonly: false,
            },
        );
        let result = builtin_cd(&["-".to_string()], &mut state).unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(state.cwd, "/home/user");
        assert!(result.stdout.contains("/home/user"));
    }

    #[test]
    fn export_and_list() {
        let mut state = make_state();
        builtin_export(&["FOO=bar".to_string()], &mut state).unwrap();
        assert!(state.env.get("FOO").unwrap().exported);
        assert_eq!(state.env.get("FOO").unwrap().value, "bar");
    }

    #[test]
    fn unset_variable() {
        let mut state = make_state();
        set_variable(&mut state, "FOO", "bar".to_string()).unwrap();
        builtin_unset(&["FOO".to_string()], &mut state).unwrap();
        assert!(state.env.get("FOO").is_none());
    }

    #[test]
    fn unset_readonly_fails() {
        let mut state = make_state();
        state.env.insert(
            "FOO".to_string(),
            Variable {
                value: "bar".to_string(),
                exported: false,
                readonly: true,
            },
        );
        let result = builtin_unset(&["FOO".to_string()], &mut state).unwrap();
        assert_eq!(result.exit_code, 1);
        assert!(state.env.contains_key("FOO"));
    }

    #[test]
    fn set_positional_params() {
        let mut state = make_state();
        builtin_set(
            &[
                "--".to_string(),
                "a".to_string(),
                "b".to_string(),
                "c".to_string(),
            ],
            &mut state,
        )
        .unwrap();
        assert_eq!(state.positional_params, vec!["a", "b", "c"]);
    }

    #[test]
    fn set_errexit() {
        let mut state = make_state();
        builtin_set(&["-e".to_string()], &mut state).unwrap();
        assert!(state.shell_opts.errexit);
        builtin_set(&["+e".to_string()], &mut state).unwrap();
        assert!(!state.shell_opts.errexit);
    }

    #[test]
    fn shift_params() {
        let mut state = make_state();
        state.positional_params = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        builtin_shift(&[], &mut state).unwrap();
        assert_eq!(state.positional_params, vec!["b", "c"]);
    }

    #[test]
    fn shift_too_many() {
        let mut state = make_state();
        state.positional_params = vec!["a".to_string()];
        let result = builtin_shift(&["5".to_string()], &mut state).unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn readonly_variable() {
        let mut state = make_state();
        builtin_readonly(&["FOO=bar".to_string()], &mut state).unwrap();
        assert!(state.env.get("FOO").unwrap().readonly);
        assert_eq!(state.env.get("FOO").unwrap().value, "bar");
    }

    #[test]
    fn declare_readonly() {
        let mut state = make_state();
        builtin_declare(&["-r".to_string(), "X=42".to_string()], &mut state).unwrap();
        assert!(state.env.get("X").unwrap().readonly);
    }

    #[test]
    fn read_single_var() {
        let mut state = make_state();
        builtin_read(&["NAME".to_string()], &mut state, "hello world\n").unwrap();
        assert_eq!(state.env.get("NAME").unwrap().value, "hello world");
    }

    #[test]
    fn read_multiple_vars() {
        let mut state = make_state();
        builtin_read(
            &["A".to_string(), "B".to_string()],
            &mut state,
            "one two three\n",
        )
        .unwrap();
        assert_eq!(state.env.get("A").unwrap().value, "one");
        assert_eq!(state.env.get("B").unwrap().value, "two three");
    }

    #[test]
    fn read_reply_default() {
        let mut state = make_state();
        builtin_read(&[], &mut state, "test input\n").unwrap();
        assert_eq!(state.env.get("REPLY").unwrap().value, "test input");
    }

    #[test]
    fn read_eof_returns_1() {
        let mut state = make_state();
        let result = builtin_read(&["VAR".to_string()], &mut state, "").unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn resolve_relative_path() {
        assert_eq!(resolve_path("/home/user", "docs"), "/home/user/docs");
        assert_eq!(resolve_path("/home/user", ".."), "/home");
        assert_eq!(resolve_path("/home/user", "/tmp"), "/tmp");
    }
}
