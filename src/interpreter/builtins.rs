//! Shell builtins that modify interpreter state.

use crate::commands::CommandMeta;
use crate::error::RustBashError;
use crate::interpreter::walker::execute_program;
use crate::interpreter::{
    ControlFlow, ExecResult, InterpreterState, Variable, VariableAttrs, VariableValue, parse,
    set_array_element, set_variable,
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
        "declare" | "typeset" => builtin_declare(args, state).map(Some),
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
        "shopt" => builtin_shopt(args, state).map(Some),
        "type" => builtin_type(args, state).map(Some),
        "command" => builtin_command(args, state, stdin).map(Some),
        "builtin" => builtin_builtin(args, state, stdin).map(Some),
        "getopts" => builtin_getopts(args, state).map(Some),
        "mapfile" | "readarray" => builtin_mapfile(args, state, stdin).map(Some),
        "pushd" => builtin_pushd(args, state).map(Some),
        "popd" => builtin_popd(args, state).map(Some),
        "dirs" => builtin_dirs(args, state).map(Some),
        "hash" => builtin_hash(args, state).map(Some),
        "wait" => Ok(Some(ExecResult::default())),
        "alias" => builtin_alias(args, state).map(Some),
        "unalias" => builtin_unalias(args, state).map(Some),
        "printf" => builtin_printf(args, state).map(Some),
        "sh" | "bash" => builtin_sh(args, state, stdin).map(Some),
        "help" => builtin_help(args, state).map(Some),
        "history" => Ok(Some(ExecResult::default())),
        _ => Ok(None),
    }
}

/// Check if a name is a known shell builtin.
/// Derives from `builtin_names()` to keep a single source of truth.
pub(crate) fn is_builtin(name: &str) -> bool {
    builtin_names().contains(&name)
}

/// Shared --help check for `command` and `builtin` wrappers.
fn check_help(name: &str, state: &InterpreterState) -> Option<ExecResult> {
    if let Some(meta) = builtin_meta(name)
        && meta.supports_help_flag
    {
        return Some(ExecResult {
            stdout: crate::commands::format_help(meta),
            ..ExecResult::default()
        });
    }
    if let Some(cmd) = state.commands.get(name)
        && let Some(meta) = cmd.meta()
        && meta.supports_help_flag
    {
        return Some(ExecResult {
            stdout: crate::commands::format_help(meta),
            ..ExecResult::default()
        });
    }
    None
}

/// Return the list of known shell builtin names.
pub fn builtin_names() -> &'static [&'static str] {
    &[
        "exit",
        "cd",
        "export",
        "unset",
        "set",
        "shift",
        "readonly",
        "declare",
        "typeset",
        "read",
        "eval",
        "source",
        ".",
        "break",
        "continue",
        ":",
        "colon",
        "let",
        "local",
        "return",
        "trap",
        "shopt",
        "type",
        "command",
        "builtin",
        "getopts",
        "mapfile",
        "readarray",
        "pushd",
        "popd",
        "dirs",
        "hash",
        "wait",
        "alias",
        "unalias",
        "printf",
        "exec",
        "sh",
        "bash",
        "help",
        "history",
    ]
}

// ── Builtin command metadata for --help ──────────────────────────────

static CD_META: CommandMeta = CommandMeta {
    name: "cd",
    synopsis: "cd [dir]",
    description: "Change the shell working directory.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static EXIT_META: CommandMeta = CommandMeta {
    name: "exit",
    synopsis: "exit [n]",
    description: "Exit the shell.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static EXPORT_META: CommandMeta = CommandMeta {
    name: "export",
    synopsis: "export [-n] [name[=value] ...]",
    description: "Set export attribute for shell variables.",
    options: &[("-n", "remove the export property from each name")],
    supports_help_flag: true,
    flags: &[],
};

static UNSET_META: CommandMeta = CommandMeta {
    name: "unset",
    synopsis: "unset [-fv] [name ...]",
    description: "Unset values and attributes of shell variables and functions.",
    options: &[
        ("-f", "treat each name as a shell function"),
        ("-v", "treat each name as a shell variable"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static SET_META: CommandMeta = CommandMeta {
    name: "set",
    synopsis: "set [-euxvnCaf] [-o option-name] [--] [arg ...]",
    description: "Set or unset values of shell options and positional parameters.",
    options: &[
        (
            "-e",
            "exit immediately if a command exits with non-zero status",
        ),
        ("-u", "treat unset variables as an error"),
        (
            "-x",
            "print commands and their arguments as they are executed",
        ),
        ("-v", "print shell input lines as they are read"),
        ("-n", "read commands but do not execute them"),
        ("-C", "do not allow output redirection to overwrite files"),
        ("-a", "mark variables for export"),
        ("-f", "disable file name generation (globbing)"),
        (
            "-o OPTION",
            "set option by name (errexit, nounset, pipefail, ...)",
        ),
    ],
    supports_help_flag: true,
    flags: &[],
};

static SHIFT_META: CommandMeta = CommandMeta {
    name: "shift",
    synopsis: "shift [n]",
    description: "Shift positional parameters.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static READONLY_META: CommandMeta = CommandMeta {
    name: "readonly",
    synopsis: "readonly [name[=value] ...]",
    description: "Mark shell variables as unchangeable.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static DECLARE_META: CommandMeta = CommandMeta {
    name: "declare",
    synopsis: "declare [-aAilnprux] [name[=value] ...]",
    description: "Set variable values and attributes.",
    options: &[
        ("-a", "indexed array"),
        ("-A", "associative array"),
        ("-i", "integer attribute"),
        ("-l", "convert to lower case on assignment"),
        ("-u", "convert to upper case on assignment"),
        ("-n", "nameref attribute"),
        ("-r", "readonly attribute"),
        ("-x", "export attribute"),
        ("-p", "display attributes and values"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static READ_META: CommandMeta = CommandMeta {
    name: "read",
    synopsis: "read [-r] [-a array] [-d delim] [-n count] [-N count] [-p prompt] [name ...]",
    description: "Read a line from standard input and split it into fields.",
    options: &[
        ("-r", "do not allow backslashes to escape characters"),
        ("-a ARRAY", "assign words to indices of ARRAY"),
        ("-d DELIM", "read until DELIM instead of newline"),
        ("-n COUNT", "read at most COUNT characters"),
        ("-N COUNT", "read exactly COUNT characters"),
        ("-p PROMPT", "output PROMPT before reading"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static EVAL_META: CommandMeta = CommandMeta {
    name: "eval",
    synopsis: "eval [arg ...]",
    description: "Execute arguments as a shell command.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static SOURCE_META: CommandMeta = CommandMeta {
    name: "source",
    synopsis: "source filename [arguments]",
    description: "Execute commands from a file in the current shell.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static BREAK_META: CommandMeta = CommandMeta {
    name: "break",
    synopsis: "break [n]",
    description: "Exit for, while, or until loops.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static CONTINUE_META: CommandMeta = CommandMeta {
    name: "continue",
    synopsis: "continue [n]",
    description: "Resume the next iteration of the enclosing loop.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static COLON_META: CommandMeta = CommandMeta {
    name: ":",
    synopsis: ": [arguments]",
    description: "No effect; the command does nothing.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static LET_META: CommandMeta = CommandMeta {
    name: "let",
    synopsis: "let arg [arg ...]",
    description: "Evaluate arithmetic expressions.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static LOCAL_META: CommandMeta = CommandMeta {
    name: "local",
    synopsis: "local [name[=value] ...]",
    description: "Define local variables.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static RETURN_META: CommandMeta = CommandMeta {
    name: "return",
    synopsis: "return [n]",
    description: "Return from a shell function.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static TRAP_META: CommandMeta = CommandMeta {
    name: "trap",
    synopsis: "trap [-lp] [action signal ...]",
    description: "Trap signals and other events.",
    options: &[
        ("-l", "list signal names"),
        ("-p", "display trap commands for each signal"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static SHOPT_META: CommandMeta = CommandMeta {
    name: "shopt",
    synopsis: "shopt [-pqsu] [optname ...]",
    description: "Set and unset shell options.",
    options: &[
        ("-s", "enable (set) each optname"),
        ("-u", "disable (unset) each optname"),
        (
            "-q",
            "suppresses normal output; exit status indicates match",
        ),
        ("-p", "display in a form that may be reused as input"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static TYPE_META: CommandMeta = CommandMeta {
    name: "type",
    synopsis: "type [-tap] name [name ...]",
    description: "Display information about command type.",
    options: &[
        ("-t", "print a single word describing the type"),
        ("-a", "display all locations containing an executable"),
        ("-p", "print the file name of the disk file"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static COMMAND_META: CommandMeta = CommandMeta {
    name: "command",
    synopsis: "command [-vVp] command [arg ...]",
    description: "Execute a simple command or display information about commands.",
    options: &[
        ("-v", "display a description of COMMAND similar to type"),
        ("-V", "display a more verbose description"),
        ("-p", "use a default value for PATH"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static BUILTIN_CMD_META: CommandMeta = CommandMeta {
    name: "builtin",
    synopsis: "builtin shell-builtin [arguments]",
    description: "Execute shell builtins.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static GETOPTS_META: CommandMeta = CommandMeta {
    name: "getopts",
    synopsis: "getopts optstring name [arg ...]",
    description: "Parse option arguments.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static MAPFILE_META: CommandMeta = CommandMeta {
    name: "mapfile",
    synopsis: "mapfile [-t] [-d delim] [-n count] [-s count] [array]",
    description: "Read lines from standard input into an indexed array variable.",
    options: &[
        ("-t", "remove a trailing delimiter from each line"),
        (
            "-d DELIM",
            "use DELIM to terminate lines instead of newline",
        ),
        ("-n COUNT", "copy at most COUNT lines"),
        ("-s COUNT", "discard the first COUNT lines"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static PUSHD_META: CommandMeta = CommandMeta {
    name: "pushd",
    synopsis: "pushd [+N | -N | dir]",
    description: "Add directories to stack.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static POPD_META: CommandMeta = CommandMeta {
    name: "popd",
    synopsis: "popd [+N | -N]",
    description: "Remove directories from stack.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static DIRS_META: CommandMeta = CommandMeta {
    name: "dirs",
    synopsis: "dirs [-cpvl]",
    description: "Display directory stack.",
    options: &[
        ("-c", "clear the directory stack"),
        ("-p", "print one entry per line"),
        ("-v", "print one entry per line, with index"),
        ("-l", "use full pathnames"),
    ],
    supports_help_flag: true,
    flags: &[],
};

static HASH_META: CommandMeta = CommandMeta {
    name: "hash",
    synopsis: "hash [-r] [name ...]",
    description: "Remember or display program locations.",
    options: &[("-r", "forget all remembered locations")],
    supports_help_flag: true,
    flags: &[],
};

static WAIT_META: CommandMeta = CommandMeta {
    name: "wait",
    synopsis: "wait [pid ...]",
    description: "Wait for job completion and return exit status.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static ALIAS_META: CommandMeta = CommandMeta {
    name: "alias",
    synopsis: "alias [-p] [name[=value] ...]",
    description: "Define or display aliases.",
    options: &[("-p", "print all defined aliases in a reusable format")],
    supports_help_flag: true,
    flags: &[],
};

static UNALIAS_META: CommandMeta = CommandMeta {
    name: "unalias",
    synopsis: "unalias [-a] name [name ...]",
    description: "Remove alias definitions.",
    options: &[("-a", "remove all alias definitions")],
    supports_help_flag: true,
    flags: &[],
};

static PRINTF_META: CommandMeta = CommandMeta {
    name: "printf",
    synopsis: "printf [-v var] format [arguments]",
    description: "Format and print data.",
    options: &[("-v VAR", "assign the output to shell variable VAR")],
    supports_help_flag: true,
    flags: &[],
};

static EXEC_META: CommandMeta = CommandMeta {
    name: "exec",
    synopsis: "exec [-a name] [command [arguments]]",
    description: "Replace the shell with the given command.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static SH_META: CommandMeta = CommandMeta {
    name: "sh",
    synopsis: "sh [-c command_string] [file]",
    description: "Execute commands from a string, file, or standard input.",
    options: &[("-c", "read commands from the command_string operand")],
    supports_help_flag: true,
    flags: &[],
};

static HELP_META: CommandMeta = CommandMeta {
    name: "help",
    synopsis: "help [pattern]",
    description: "Display information about builtin commands.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

static HISTORY_META: CommandMeta = CommandMeta {
    name: "history",
    synopsis: "history [n]",
    description: "Display the command history list.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

/// Return the `CommandMeta` for a shell builtin, if one exists.
pub(crate) fn builtin_meta(name: &str) -> Option<&'static CommandMeta> {
    match name {
        "cd" => Some(&CD_META),
        "exit" => Some(&EXIT_META),
        "export" => Some(&EXPORT_META),
        "unset" => Some(&UNSET_META),
        "set" => Some(&SET_META),
        "shift" => Some(&SHIFT_META),
        "readonly" => Some(&READONLY_META),
        "declare" | "typeset" => Some(&DECLARE_META),
        "read" => Some(&READ_META),
        "eval" => Some(&EVAL_META),
        "source" | "." => Some(&SOURCE_META),
        "break" => Some(&BREAK_META),
        "continue" => Some(&CONTINUE_META),
        ":" | "colon" => Some(&COLON_META),
        "let" => Some(&LET_META),
        "local" => Some(&LOCAL_META),
        "return" => Some(&RETURN_META),
        "trap" => Some(&TRAP_META),
        "shopt" => Some(&SHOPT_META),
        "type" => Some(&TYPE_META),
        "command" => Some(&COMMAND_META),
        "builtin" => Some(&BUILTIN_CMD_META),
        "getopts" => Some(&GETOPTS_META),
        "mapfile" | "readarray" => Some(&MAPFILE_META),
        "pushd" => Some(&PUSHD_META),
        "popd" => Some(&POPD_META),
        "dirs" => Some(&DIRS_META),
        "hash" => Some(&HASH_META),
        "wait" => Some(&WAIT_META),
        "alias" => Some(&ALIAS_META),
        "unalias" => Some(&UNALIAS_META),
        "printf" => Some(&PRINTF_META),
        "exec" => Some(&EXEC_META),
        "sh" | "bash" => Some(&SH_META),
        "help" => Some(&HELP_META),
        "history" => Some(&HISTORY_META),
        _ => None,
    }
}

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
            Some(v) if !v.value.as_scalar().is_empty() => v.value.as_scalar().to_string(),
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
            Some(v) if !v.value.as_scalar().is_empty() => v.value.as_scalar().to_string(),
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
        var.attrs.insert(VariableAttrs::EXPORTED);
    }
    let new_cwd = state.cwd.clone();
    let _ = set_variable(state, "PWD", new_cwd);
    if let Some(var) = state.env.get_mut("PWD") {
        var.attrs.insert(VariableAttrs::EXPORTED);
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
    if args.is_empty() || args == ["-p"] {
        // List all exported variables
        let mut lines: Vec<String> = state
            .env
            .iter()
            .filter(|(_, v)| v.exported())
            .map(|(k, v)| format_declare_line(k, v))
            .collect();
        lines.sort();
        return Ok(ExecResult {
            stdout: lines.join(""),
            ..ExecResult::default()
        });
    }

    let mut unexport = false;
    for arg in args {
        if arg == "-n" {
            unexport = true;
            continue;
        }
        if arg.starts_with('-') && !arg.contains('=') {
            continue; // skip other flags
        }
        if let Some((name, value)) = arg.split_once("+=") {
            // export name+=value — append
            let current = state
                .env
                .get(name)
                .map(|v| v.value.as_scalar().to_string())
                .unwrap_or_default();
            let new_val = format!("{current}{value}");
            set_variable(state, name, new_val)?;
            if let Some(var) = state.env.get_mut(name) {
                var.attrs.insert(VariableAttrs::EXPORTED);
            }
        } else if let Some((name, value)) = arg.split_once('=') {
            set_variable(state, name, value.to_string())?;
            if let Some(var) = state.env.get_mut(name) {
                if unexport {
                    var.attrs.remove(VariableAttrs::EXPORTED);
                } else {
                    var.attrs.insert(VariableAttrs::EXPORTED);
                }
            }
        } else if unexport {
            // export -n VAR — remove export flag
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                var.attrs.remove(VariableAttrs::EXPORTED);
            }
        } else {
            // Just mark existing variable as exported
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                var.attrs.insert(VariableAttrs::EXPORTED);
            } else {
                // Create empty exported variable
                state.env.insert(
                    arg.clone(),
                    Variable {
                        value: VariableValue::Scalar(String::new()),
                        attrs: VariableAttrs::EXPORTED,
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
    let mut unset_func = false;
    let mut names_start = 0;
    for (i, arg) in args.iter().enumerate() {
        if arg == "-f" {
            unset_func = true;
            names_start = i + 1;
        } else if arg == "-v" {
            unset_func = false;
            names_start = i + 1;
        } else if arg.starts_with('-') {
            names_start = i + 1;
        } else {
            break;
        }
    }
    for arg in &args[names_start..] {
        if unset_func {
            state.functions.remove(arg.as_str());
            continue;
        }
        // Check for array element unset: name[index]
        if let Some(bracket_pos) = arg.find('[')
            && arg.ends_with(']')
        {
            let name = &arg[..bracket_pos];
            let index_str = &arg[bracket_pos + 1..arg.len() - 1];
            if let Some(var) = state.env.get(name)
                && var.readonly()
            {
                return Ok(ExecResult {
                    stderr: format!("unset: {name}: cannot unset: readonly variable\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
            // Evaluate index before borrowing env mutably
            let is_indexed = state
                .env
                .get(name)
                .is_some_and(|v| matches!(v.value, VariableValue::IndexedArray(_)));
            let is_assoc = state
                .env
                .get(name)
                .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)));
            let is_scalar = state
                .env
                .get(name)
                .is_some_and(|v| matches!(v.value, VariableValue::Scalar(_)));

            if is_indexed {
                if let Ok(idx) = crate::interpreter::arithmetic::eval_arithmetic(index_str, state) {
                    let actual_idx = if idx < 0 {
                        // Resolve negative index relative to max key.
                        let max_key = state.env.get(name).and_then(|v| {
                            if let VariableValue::IndexedArray(map) = &v.value {
                                map.keys().next_back().copied()
                            } else {
                                None
                            }
                        });
                        if let Some(mk) = max_key {
                            let resolved = mk as i64 + 1 + idx;
                            if resolved < 0 {
                                return Ok(ExecResult {
                                    stderr: format!(
                                        "unset: {name}[{index_str}]: bad array subscript\n"
                                    ),
                                    exit_code: 1,
                                    ..ExecResult::default()
                                });
                            }
                            Some(resolved as usize)
                        } else {
                            None
                        }
                    } else {
                        Some(idx as usize)
                    };
                    if let Some(actual) = actual_idx
                        && let Some(var) = state.env.get_mut(name)
                        && let VariableValue::IndexedArray(map) = &mut var.value
                    {
                        map.remove(&actual);
                    }
                }
            } else if is_assoc {
                // Expand variables and strip quotes in the key.
                let word = brush_parser::ast::Word {
                    value: index_str.to_string(),
                    loc: None,
                };
                let expanded_key =
                    crate::interpreter::expansion::expand_word_to_string_mut(&word, state)?;
                if let Some(var) = state.env.get_mut(name)
                    && let VariableValue::AssociativeArray(map) = &mut var.value
                {
                    map.remove(&expanded_key);
                }
            } else if is_scalar
                && index_str == "0"
                && let Some(var) = state.env.get_mut(name)
            {
                var.value = VariableValue::Scalar(String::new());
            }
            continue;
        }
        if let Some(var) = state.env.get(arg.as_str())
            && var.readonly()
        {
            return Ok(ExecResult {
                stderr: format!("unset: {arg}: cannot unset: readonly variable\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
        }
        // Resolve nameref: unset the target, not the ref itself.
        let is_nameref = state
            .env
            .get(arg.as_str())
            .is_some_and(|v| v.attrs.contains(VariableAttrs::NAMEREF));
        if is_nameref {
            let target = crate::interpreter::resolve_nameref_or_self(arg, state);
            if target != *arg {
                // Check if target is readonly.
                if let Some(var) = state.env.get(target.as_str())
                    && var.readonly()
                {
                    return Ok(ExecResult {
                        stderr: format!("unset: {target}: cannot unset: readonly variable\n"),
                        exit_code: 1,
                        ..ExecResult::default()
                    });
                }
                state.env.remove(target.as_str());
                continue;
            }
        }
        state.env.remove(arg.as_str());
    }
    Ok(ExecResult::default())
}

// ── set ─────────────────────────────────────────────────────────────

fn builtin_set(args: &[String], state: &mut InterpreterState) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // List all variables in bash format (no quotes for scalars, array syntax for arrays)
        let mut lines: Vec<String> = state
            .env
            .iter()
            .map(|(k, v)| match &v.value {
                VariableValue::IndexedArray(map) => {
                    let elements: Vec<String> = map
                        .iter()
                        .map(|(idx, val)| format!("[{idx}]=\"{val}\""))
                        .collect();
                    format!("{k}=({})\n", elements.join(" "))
                }
                VariableValue::AssociativeArray(map) => {
                    let elements: Vec<String> = map
                        .iter()
                        .map(|(key, val)| {
                            if key.contains(' ') || key.contains('"') {
                                format!("[\"{key}\"]=\"{val}\"")
                            } else {
                                format!("[{key}]=\"{val}\"")
                            }
                        })
                        .collect();
                    format!("{k}=({})\n", elements.join(" "))
                }
                VariableValue::Scalar(s) => format!("{k}={s}\n"),
            })
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
                    // set +o with no arg: list options in re-parseable format
                    let mut out = String::new();
                    for name in SET_O_OPTIONS {
                        let val = get_set_option(name, state).unwrap_or(false);
                        let flag = if val { "-o" } else { "+o" };
                        out.push_str(&format!("set {flag} {name}\n"));
                    }
                    return Ok(ExecResult {
                        stdout: out,
                        ..ExecResult::default()
                    });
                } else {
                    // set -o with no arg: list options in tabular format
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
        'v' => state.shell_opts.verbose = enable,
        'n' => state.shell_opts.noexec = enable,
        'C' => state.shell_opts.noclobber = enable,
        'a' => state.shell_opts.allexport = enable,
        'f' => state.shell_opts.noglob = enable,
        _ => {}
    }
}

fn apply_option_name(name: &str, enable: bool, state: &mut InterpreterState) {
    match name {
        "errexit" => state.shell_opts.errexit = enable,
        "nounset" => state.shell_opts.nounset = enable,
        "pipefail" => state.shell_opts.pipefail = enable,
        "xtrace" => state.shell_opts.xtrace = enable,
        "verbose" => state.shell_opts.verbose = enable,
        "noexec" => state.shell_opts.noexec = enable,
        "noclobber" => state.shell_opts.noclobber = enable,
        "allexport" => state.shell_opts.allexport = enable,
        "noglob" => state.shell_opts.noglob = enable,
        "posix" => state.shell_opts.posix = enable,
        "vi" => state.shell_opts.vi_mode = enable,
        "emacs" => state.shell_opts.emacs_mode = enable,
        _ => {}
    }
}

fn format_options(state: &InterpreterState) -> String {
    let mut out = String::new();
    for name in SET_O_OPTIONS {
        let val = get_set_option(name, state).unwrap_or(false);
        let status = if val { "on" } else { "off" };
        out.push_str(&format!("{name:<23}\t{status}\n"));
    }
    out
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
    if args.is_empty() || args == ["-p"] {
        let mut lines: Vec<String> = state
            .env
            .iter()
            .filter(|(_, v)| v.readonly())
            .map(|(k, v)| format_declare_line(k, v))
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
        if let Some((name, value)) = arg.split_once("+=") {
            let current = state
                .env
                .get(name)
                .map(|v| v.value.as_scalar().to_string())
                .unwrap_or_default();
            let new_val = format!("{current}{value}");
            set_variable(state, name, new_val)?;
            if let Some(var) = state.env.get_mut(name) {
                var.attrs.insert(VariableAttrs::READONLY);
            }
        } else if let Some((name, value)) = arg.split_once('=') {
            set_variable(state, name, value.to_string())?;
            if let Some(var) = state.env.get_mut(name) {
                var.attrs.insert(VariableAttrs::READONLY);
            }
        } else {
            // Mark existing variable as readonly
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                var.attrs.insert(VariableAttrs::READONLY);
            } else {
                state.env.insert(
                    arg.clone(),
                    Variable {
                        value: VariableValue::Scalar(String::new()),
                        attrs: VariableAttrs::READONLY,
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
    let mut make_indexed_array = false;
    let mut make_assoc_array = false;
    let mut make_integer = false;
    let mut make_lowercase = false;
    let mut make_uppercase = false;
    let mut make_nameref = false;
    let mut print_mode = false;
    let mut func_mode = false; // -f: functions
    let mut func_names_mode = false; // -F: function names only
    let mut global_mode = false; // -g
    let mut remove_exported = false; // +x
    let mut var_args: Vec<&String> = Vec::new();

    for arg in args {
        if let Some(flags) = arg.strip_prefix('-') {
            if flags.is_empty() {
                var_args.push(arg);
                continue;
            }
            for c in flags.chars() {
                match c {
                    'r' => make_readonly = true,
                    'x' => make_exported = true,
                    'a' => make_indexed_array = true,
                    'A' => make_assoc_array = true,
                    'i' => make_integer = true,
                    'l' => make_lowercase = true,
                    'u' => make_uppercase = true,
                    'n' => make_nameref = true,
                    'p' => print_mode = true,
                    'f' => func_mode = true,
                    'F' => func_names_mode = true,
                    'g' => global_mode = true,
                    _ => {}
                }
            }
        } else if let Some(flags) = arg.strip_prefix('+') {
            for c in flags.chars() {
                if c == 'x' {
                    remove_exported = true;
                }
            }
        } else {
            var_args.push(arg);
        }
    }

    // declare -f / declare -F: function listing/checking
    if func_mode || func_names_mode {
        return declare_functions(state, &var_args, func_names_mode);
    }

    // declare -p [varname...] — print variable declarations
    if print_mode {
        return declare_print(
            state,
            &var_args,
            make_readonly,
            make_exported,
            make_nameref,
            make_indexed_array,
            make_assoc_array,
        );
    }

    // typeset +x name — remove export attribute
    if remove_exported {
        for arg in &var_args {
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                var.attrs.remove(VariableAttrs::EXPORTED);
            }
        }
        return Ok(ExecResult::default());
    }

    let _ = global_mode; // accepted but not yet meaningful (no dynamic scoping)

    let has_any_flag = make_readonly
        || make_exported
        || make_indexed_array
        || make_assoc_array
        || make_integer
        || make_lowercase
        || make_uppercase
        || make_nameref;

    if var_args.is_empty() && !has_any_flag {
        // declare with no args — list all variables
        return declare_list_all(state);
    }

    // Build the attribute bitmask from flags.
    let mut flag_attrs = VariableAttrs::empty();
    if make_readonly {
        flag_attrs.insert(VariableAttrs::READONLY);
    }
    if make_exported {
        flag_attrs.insert(VariableAttrs::EXPORTED);
    }
    if make_integer {
        flag_attrs.insert(VariableAttrs::INTEGER);
    }
    if make_lowercase {
        flag_attrs.insert(VariableAttrs::LOWERCASE);
    }
    if make_uppercase {
        flag_attrs.insert(VariableAttrs::UPPERCASE);
    }
    if make_nameref {
        flag_attrs.insert(VariableAttrs::NAMEREF);
    }

    for arg in var_args {
        // Check for += (append) before = (assign)
        if let Some((name, value)) = arg.split_once("+=") {
            declare_append_value(
                state,
                name,
                value,
                flag_attrs,
                make_assoc_array,
                make_indexed_array,
            )?;
        } else if let Some((name, value)) = arg.split_once('=') {
            declare_with_value(
                state,
                name,
                value,
                flag_attrs,
                make_assoc_array,
                make_indexed_array,
                make_nameref,
            )?;
        } else {
            declare_without_value(state, arg, flag_attrs, make_assoc_array, make_indexed_array)?;
        }
    }

    Ok(ExecResult::default())
}

/// Handle `declare -f` (list function bodies) and `declare -F` (list function names).
fn declare_functions(
    state: &InterpreterState,
    var_args: &[&String],
    names_only: bool,
) -> Result<ExecResult, RustBashError> {
    if var_args.is_empty() {
        // List all functions
        let mut lines: Vec<String> = Vec::new();
        for name in state.functions.keys() {
            if names_only {
                lines.push(format!("declare -f {name}\n"));
            } else {
                lines.push(format!("{name} () {{ :; }}\n")); // simplified body
            }
        }
        lines.sort();
        return Ok(ExecResult {
            stdout: lines.join(""),
            ..ExecResult::default()
        });
    }
    // Check specific function existence
    let mut exit_code = 0;
    let mut stdout = String::new();
    for name in var_args {
        if state.functions.contains_key(name.as_str()) {
            if names_only {
                stdout.push_str(&format!("declare -f {name}\n"));
            }
        } else {
            exit_code = 1;
        }
    }
    Ok(ExecResult {
        stdout,
        exit_code,
        ..ExecResult::default()
    })
}

/// Print variable declarations with `declare -p`.
fn declare_print(
    state: &InterpreterState,
    var_args: &[&String],
    filter_readonly: bool,
    filter_exported: bool,
    filter_nameref: bool,
    filter_indexed: bool,
    filter_assoc: bool,
) -> Result<ExecResult, RustBashError> {
    let has_filter =
        filter_readonly || filter_exported || filter_nameref || filter_indexed || filter_assoc;

    if var_args.is_empty() {
        if has_filter {
            // Filter by attribute
            let mut lines: Vec<String> = state
                .env
                .iter()
                .filter(|(_, v)| {
                    if filter_readonly && v.attrs.contains(VariableAttrs::READONLY) {
                        return true;
                    }
                    if filter_exported && v.attrs.contains(VariableAttrs::EXPORTED) {
                        return true;
                    }
                    if filter_nameref && v.attrs.contains(VariableAttrs::NAMEREF) {
                        return true;
                    }
                    if filter_indexed && matches!(v.value, VariableValue::IndexedArray(_)) {
                        return true;
                    }
                    if filter_assoc && matches!(v.value, VariableValue::AssociativeArray(_)) {
                        return true;
                    }
                    false
                })
                .map(|(k, v)| format_declare_line(k, v))
                .collect();
            lines.sort();
            return Ok(ExecResult {
                stdout: lines.join(""),
                ..ExecResult::default()
            });
        }
        return declare_list_all(state);
    }
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0;
    for name in var_args {
        if let Some(var) = state.env.get(name.as_str()) {
            stdout.push_str(&format_declare_line(name, var));
        } else {
            stderr.push_str(&format!("declare: {name}: not found\n"));
            exit_code = 1;
        }
    }
    Ok(ExecResult {
        stdout,
        stderr,
        exit_code,
        stdout_bytes: None,
    })
}

/// List all variables with their declarations.
fn declare_list_all(state: &InterpreterState) -> Result<ExecResult, RustBashError> {
    let mut lines: Vec<String> = state
        .env
        .iter()
        .map(|(k, v)| format_declare_line(k, v))
        .collect();
    lines.sort();
    Ok(ExecResult {
        stdout: lines.join(""),
        ..ExecResult::default()
    })
}

/// Format a single `declare -<flags> name="value"` line.
fn format_declare_line(name: &str, var: &Variable) -> String {
    let mut flags = String::new();
    // Flag order: a, A, i, l, n, r, u, x (alphabetical)
    if matches!(var.value, VariableValue::IndexedArray(_)) {
        flags.push('a');
    }
    if matches!(var.value, VariableValue::AssociativeArray(_)) {
        flags.push('A');
    }
    if var.attrs.contains(VariableAttrs::INTEGER) {
        flags.push('i');
    }
    if var.attrs.contains(VariableAttrs::LOWERCASE) {
        flags.push('l');
    }
    if var.attrs.contains(VariableAttrs::NAMEREF) {
        flags.push('n');
    }
    if var.attrs.contains(VariableAttrs::READONLY) {
        flags.push('r');
    }
    if var.attrs.contains(VariableAttrs::UPPERCASE) {
        flags.push('u');
    }
    if var.attrs.contains(VariableAttrs::EXPORTED) {
        flags.push('x');
    }

    let flag_str = if flags.is_empty() {
        "-- ".to_string()
    } else {
        format!("-{flags} ")
    };

    match &var.value {
        VariableValue::Scalar(s) => format!("declare {flag_str}{name}=\"{s}\"\n"),
        VariableValue::IndexedArray(map) => {
            let elems: Vec<String> = map.iter().map(|(k, v)| format!("[{k}]=\"{v}\"")).collect();
            format!("declare {flag_str}{name}=({})\n", elems.join(" "))
        }
        VariableValue::AssociativeArray(map) => {
            let mut elems: Vec<String> =
                map.iter().map(|(k, v)| format!("[{k}]=\"{v}\"")).collect();
            elems.sort();
            format!("declare {flag_str}{name}=({})\n", elems.join(" "))
        }
    }
}

/// Handle `declare [-flags] name+=value` — append to existing variable.
fn declare_append_value(
    state: &mut InterpreterState,
    name: &str,
    value: &str,
    flag_attrs: VariableAttrs,
    make_assoc_array: bool,
    _make_indexed_array: bool,
) -> Result<(), RustBashError> {
    // Handle array append: name+=(val1 val2 ...)
    if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        // Check if the target is an assoc array
        let is_assoc = make_assoc_array
            || state
                .env
                .get(name)
                .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)));

        if is_assoc {
            // Create assoc array if it doesn't exist
            if !state.env.contains_key(name) {
                state.env.insert(
                    name.to_string(),
                    Variable {
                        value: VariableValue::AssociativeArray(std::collections::BTreeMap::new()),
                        attrs: flag_attrs,
                    },
                );
            }
            parse_and_set_assoc_array_append(state, name, inner)?;
        } else {
            // Find current max index + 1
            let start_idx = match state.env.get(name) {
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

            // Create array if it doesn't exist
            if !state.env.contains_key(name) {
                state.env.insert(
                    name.to_string(),
                    Variable {
                        value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                        attrs: flag_attrs,
                    },
                );
            }

            // Convert scalar to array if needed
            if let Some(var) = state.env.get_mut(name)
                && let VariableValue::Scalar(s) = &var.value
            {
                let mut map = std::collections::BTreeMap::new();
                if !s.is_empty() {
                    map.insert(0, s.clone());
                }
                var.value = VariableValue::IndexedArray(map);
            }

            let words = shell_split_array_body(inner);
            let mut idx = start_idx;
            for word in &words {
                let val = unquote_simple(word);
                crate::interpreter::set_array_element(state, name, idx, val)?;
                idx += 1;
            }

            if let Some(var) = state.env.get_mut(name) {
                var.attrs.insert(flag_attrs);
            }
        }
    } else {
        // Scalar append
        let current = state
            .env
            .get(name)
            .map(|v| v.value.as_scalar().to_string())
            .unwrap_or_default();
        let new_val = format!("{current}{value}");
        set_variable(state, name, new_val)?;
        if let Some(var) = state.env.get_mut(name) {
            var.attrs.insert(flag_attrs);
        }
    }
    Ok(())
}

/// Handle `declare [-flags] name=value`.
fn declare_with_value(
    state: &mut InterpreterState,
    name: &str,
    value: &str,
    flag_attrs: VariableAttrs,
    make_assoc_array: bool,
    make_indexed_array: bool,
    make_nameref: bool,
) -> Result<(), RustBashError> {
    if make_nameref {
        // Nameref: set the variable directly (don't follow existing nameref).
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::Scalar(String::new()),
                attrs: VariableAttrs::empty(),
            });
        var.value = VariableValue::Scalar(value.to_string());
        var.attrs.insert(flag_attrs);
        return Ok(());
    }

    if make_assoc_array {
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::AssociativeArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            });
        var.attrs.insert(flag_attrs);
        if !matches!(var.value, VariableValue::AssociativeArray(_)) {
            var.value = VariableValue::AssociativeArray(std::collections::BTreeMap::new());
        }
        // Parse assoc array literal: ([key1]=val1 [key2]=val2 ...)
        if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            parse_and_set_assoc_array(state, name, inner)?;
        }
    } else if make_indexed_array {
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            });
        var.attrs.insert(flag_attrs);
        if !matches!(var.value, VariableValue::IndexedArray(_)) {
            var.value = VariableValue::IndexedArray(std::collections::BTreeMap::new());
        }
        // Parse array literal (x y z) or set element [0].
        if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            parse_and_set_indexed_array(state, name, inner)?;
        } else if !value.is_empty() {
            crate::interpreter::set_array_element(state, name, 0, value.to_string())?;
        }
    } else if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        // `declare name=(x y z)` without -a flag — auto-create indexed array.
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            });
        var.attrs.insert(flag_attrs);
        if !matches!(var.value, VariableValue::IndexedArray(_)) {
            var.value = VariableValue::IndexedArray(std::collections::BTreeMap::new());
        }
        parse_and_set_indexed_array(state, name, inner)?;
    } else {
        let non_readonly_attrs = flag_attrs - VariableAttrs::READONLY;
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::Scalar(String::new()),
                attrs: VariableAttrs::empty(),
            });
        var.attrs.insert(non_readonly_attrs);
        // Now set value through set_variable to apply INTEGER/LOWERCASE/UPPERCASE.
        set_variable(state, name, value.to_string())?;
        // Apply READONLY after the value is set.
        if flag_attrs.contains(VariableAttrs::READONLY)
            && let Some(var) = state.env.get_mut(name)
        {
            var.attrs.insert(VariableAttrs::READONLY);
        }
    }
    Ok(())
}

/// Handle `declare [-flags] name` (no value).
fn declare_without_value(
    state: &mut InterpreterState,
    name: &str,
    flag_attrs: VariableAttrs,
    make_assoc_array: bool,
    make_indexed_array: bool,
) -> Result<(), RustBashError> {
    if let Some(var) = state.env.get_mut(name) {
        var.attrs.insert(flag_attrs);
        if make_assoc_array && !matches!(var.value, VariableValue::AssociativeArray(_)) {
            var.value = VariableValue::AssociativeArray(std::collections::BTreeMap::new());
        }
        if make_indexed_array && !matches!(var.value, VariableValue::IndexedArray(_)) {
            var.value = VariableValue::IndexedArray(std::collections::BTreeMap::new());
        }
    } else {
        let value = if make_assoc_array {
            VariableValue::AssociativeArray(std::collections::BTreeMap::new())
        } else if make_indexed_array {
            VariableValue::IndexedArray(std::collections::BTreeMap::new())
        } else {
            VariableValue::Scalar(String::new())
        };
        state.env.insert(
            name.to_string(),
            Variable {
                value,
                attrs: flag_attrs,
            },
        );
    }
    Ok(())
}

/// Parse an array literal body like `x y z` or `[0]="x" [1]="y"` and populate
/// the named variable as an indexed array.
fn parse_and_set_indexed_array(
    state: &mut InterpreterState,
    name: &str,
    body: &str,
) -> Result<(), RustBashError> {
    // Split into shell-like words respecting double/single quotes.
    let words = shell_split_array_body(body);
    // Reset the array to empty.
    if let Some(var) = state.env.get_mut(name) {
        var.value = VariableValue::IndexedArray(std::collections::BTreeMap::new());
    }
    let mut idx: usize = 0;
    for word in &words {
        if let Some(rest) = word.strip_prefix('[') {
            // [index]="value" form
            if let Some(eq_pos) = rest.find("]=") {
                let index_str = &rest[..eq_pos];
                let value_part = &rest[eq_pos + 2..];
                let value = unquote_simple(value_part);
                if let Ok(i) = index_str.parse::<usize>() {
                    crate::interpreter::set_array_element(state, name, i, value)?;
                    idx = i + 1;
                }
            }
        } else {
            let value = unquote_simple(word);
            crate::interpreter::set_array_element(state, name, idx, value)?;
            idx += 1;
        }
    }
    Ok(())
}

/// Parse and set associative array from literal body: `[key1]=val1 [key2]=val2 ...`
fn parse_and_set_assoc_array(
    state: &mut InterpreterState,
    name: &str,
    body: &str,
) -> Result<(), RustBashError> {
    let words = shell_split_array_body(body);
    // Reset the array to empty.
    if let Some(var) = state.env.get_mut(name) {
        var.value = VariableValue::AssociativeArray(std::collections::BTreeMap::new());
    }
    for word in &words {
        if let Some(rest) = word.strip_prefix('[') {
            // [key]=value form
            if let Some(eq_pos) = rest.find("]=") {
                let key = unquote_simple(&rest[..eq_pos]);
                let value = unquote_simple(&rest[eq_pos + 2..]);
                crate::interpreter::set_assoc_element(state, name, key, value)?;
            } else if let Some(key_str) = rest.strip_suffix(']') {
                // [key]= with empty value (no = sign) — just a key with empty value
                let key = unquote_simple(key_str);
                crate::interpreter::set_assoc_element(state, name, key, String::new())?;
            }
        }
        // Non-[key]=val entries are ignored for assoc arrays
    }
    Ok(())
}

/// Parse and append to an associative array from body text (no reset).
fn parse_and_set_assoc_array_append(
    state: &mut InterpreterState,
    name: &str,
    body: &str,
) -> Result<(), RustBashError> {
    let words = shell_split_array_body(body);
    for word in &words {
        if let Some(rest) = word.strip_prefix('[') {
            if let Some(eq_pos) = rest.find("]=") {
                let key = unquote_simple(&rest[..eq_pos]);
                let value = unquote_simple(&rest[eq_pos + 2..]);
                crate::interpreter::set_assoc_element(state, name, key, value)?;
            } else if let Some(key_str) = rest.strip_suffix(']') {
                let key = unquote_simple(key_str);
                crate::interpreter::set_assoc_element(state, name, key, String::new())?;
            }
        }
    }
    Ok(())
}

/// Simple shell word splitting for array bodies, respecting double/single quotes.
fn shell_split_array_body(s: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = s.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                chars.next();
            }
            '"' => {
                chars.next();
                current.push('"');
                while let Some(&ch) = chars.peek() {
                    if ch == '"' {
                        current.push('"');
                        chars.next();
                        break;
                    }
                    if ch == '\\' {
                        chars.next();
                        current.push('\\');
                        if let Some(&esc) = chars.peek() {
                            current.push(esc);
                            chars.next();
                        }
                    } else {
                        current.push(ch);
                        chars.next();
                    }
                }
            }
            '\'' => {
                chars.next();
                current.push('\'');
                while let Some(&ch) = chars.peek() {
                    if ch == '\'' {
                        current.push('\'');
                        chars.next();
                        break;
                    }
                    current.push(ch);
                    chars.next();
                }
            }
            _ => {
                current.push(c);
                chars.next();
            }
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Remove outer quotes from a simple value like `"foo"` or `'bar'`.
fn unquote_simple(s: &str) -> String {
    if s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')))
    {
        return s[1..s.len() - 1].to_string();
    }
    s.to_string()
}

// ── read ────────────────────────────────────────────────────────────

fn builtin_read(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let mut raw_mode = false;
    let mut array_name: Option<String> = None;
    let mut delimiter: Option<char> = None; // None = newline (default)
    let mut read_until_eof = false; // -d '' means read until EOF
    let mut n_count: Option<usize> = None; // -n count
    let mut big_n_count: Option<usize> = None; // -N count
    let mut var_names: Vec<&str> = Vec::new();
    let mut i = 0;

    // Parse arguments — support combined short flags like `-ra`
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            // Everything after -- is a variable name
            for a in &args[i + 1..] {
                var_names.push(a);
            }
            break;
        } else if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
            let flag_chars: Vec<char> = arg[1..].chars().collect();
            let mut j = 0;
            while j < flag_chars.len() {
                match flag_chars[j] {
                    'r' => raw_mode = true,
                    's' => { /* silent mode — no-op in sandbox */ }
                    'a' => {
                        // -a arrayname: rest of this flag group is the name, or next arg
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        if rest.is_empty() {
                            i += 1;
                            if i < args.len() {
                                array_name = Some(args[i].clone());
                            }
                        } else {
                            array_name = Some(rest);
                        }
                        j = flag_chars.len(); // consumed rest of flag group
                        continue;
                    }
                    'd' => {
                        // -d delim: rest of flag group is the delimiter, or next arg
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        let delim_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() { args[i].as_str() } else { "" }
                        } else {
                            rest.as_str()
                        };
                        if delim_str.is_empty() {
                            read_until_eof = true;
                        } else {
                            delimiter = Some(delim_str.chars().next().unwrap());
                        }
                        j = flag_chars.len();
                        continue;
                    }
                    'n' => {
                        // -n count
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        let count_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() {
                                args[i].as_str()
                            } else {
                                "0"
                            }
                        } else {
                            rest.as_str()
                        };
                        n_count = count_str.parse().ok();
                        j = flag_chars.len();
                        continue;
                    }
                    'N' => {
                        // -N count
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        let count_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() {
                                args[i].as_str()
                            } else {
                                "0"
                            }
                        } else {
                            rest.as_str()
                        };
                        big_n_count = count_str.parse().ok();
                        j = flag_chars.len();
                        continue;
                    }
                    'p' => {
                        // -p prompt: skip the prompt value (no-op in sandbox)
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        if rest.is_empty() {
                            i += 1; // skip the next arg (the prompt string)
                        }
                        j = flag_chars.len();
                        continue;
                    }
                    't' => {
                        // -t timeout: skip the timeout value (stub)
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        if rest.is_empty() {
                            i += 1;
                        }
                        j = flag_chars.len();
                        continue;
                    }
                    _ => { /* unknown flag — ignore */ }
                }
                j += 1;
            }
        } else {
            var_names.push(arg);
        }
        i += 1;
    }

    // Defaults
    if array_name.is_none() && var_names.is_empty() {
        var_names.push("REPLY");
    }

    // Get the remaining stdin
    let effective_stdin = if state.stdin_offset < stdin.len() {
        &stdin[state.stdin_offset..]
    } else {
        ""
    };

    // -t timeout stub: return 1 if stdin exhausted
    // (The actual timeout behavior is a no-op since stdin is always available in sandbox)

    if effective_stdin.is_empty() {
        return Ok(ExecResult {
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    // Read input based on mode, tracking whether we hit EOF without the expected terminator
    let mut hit_eof = false;

    let line = if let Some(count) = big_n_count {
        // -N count: read exactly N characters, including newlines
        let chars: String = effective_stdin.chars().take(count).collect();
        state.stdin_offset += chars.len();
        if chars.chars().count() < count {
            hit_eof = true;
        }
        chars
    } else if let Some(count) = n_count {
        // -n count: read at most N characters, stop at newline
        let mut result = String::new();
        let mut found_newline = false;
        for ch in effective_stdin.chars().take(count) {
            if ch == '\n' {
                state.stdin_offset += 1; // consume the newline
                found_newline = true;
                break;
            }
            result.push(ch);
        }
        state.stdin_offset += result.len();
        if !found_newline && state.stdin_offset >= stdin.len() {
            hit_eof = true;
        }
        result
    } else if read_until_eof {
        // -d '' : read until EOF (NUL never found in text, so always returns 1)
        hit_eof = true;
        let data = effective_stdin.to_string();
        state.stdin_offset += data.len();
        data
    } else if let Some(delim) = delimiter {
        // -d delim: read until delimiter character
        let mut result = String::new();
        let mut found_delim = false;
        for ch in effective_stdin.chars() {
            if ch == delim {
                state.stdin_offset += ch.len_utf8(); // consume the delimiter
                found_delim = true;
                break;
            }
            result.push(ch);
        }
        state.stdin_offset += result.len();
        if !found_delim {
            hit_eof = true;
        }
        result
    } else {
        // Default: read until newline
        match effective_stdin.lines().next() {
            Some(l) => {
                state.stdin_offset += l.len();
                if state.stdin_offset < stdin.len()
                    && stdin.as_bytes().get(state.stdin_offset) == Some(&b'\n')
                {
                    state.stdin_offset += 1;
                } else {
                    hit_eof = true;
                }
                l.to_string()
            }
            None => {
                return Ok(ExecResult {
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
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

    // Handle -r flag: process backslash escapes if not raw mode.
    // -N also suppresses backslash processing (bash behavior).
    let line = if raw_mode || big_n_count.is_some() {
        line
    } else {
        let mut result = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\\' {
                if let Some(&next) = chars.peek() {
                    if next == '\n' {
                        chars.next(); // skip newline (line continuation)
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

    // Get IFS for splitting
    let ifs = state
        .env
        .get("IFS")
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_else(|| " \t\n".to_string());

    if let Some(ref arr_name) = array_name {
        // -a mode: split into indexed array
        let fields: Vec<&str> = if ifs.is_empty() {
            vec![line.as_str()]
        } else {
            split_by_ifs(&line, &ifs)
        };

        // Clear or create the array
        state.env.insert(
            arr_name.to_string(),
            Variable {
                value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            },
        );

        for (idx, field) in fields.iter().enumerate() {
            set_array_element(state, arr_name, idx, field.to_string())?;
        }
    } else if big_n_count.is_some() {
        // -N mode: no IFS splitting, assign raw content directly
        let var_name = var_names.first().copied().unwrap_or("REPLY");
        set_variable(state, var_name, line)?;
        // Clear remaining variables
        for extra_var in var_names.iter().skip(1) {
            set_variable(state, extra_var, String::new())?;
        }
    } else {
        // Normal mode: assign to named variables, preserving original text for the last var
        assign_fields_to_vars(state, &line, &ifs, &var_names)?;
    }

    Ok(ExecResult {
        exit_code: i32::from(hit_eof),
        ..ExecResult::default()
    })
}

/// Assign IFS-split fields to variables, preserving original text for the last variable.
/// In bash, the last variable receives the remainder of the line (not a split-and-rejoin).
fn assign_fields_to_vars(
    state: &mut InterpreterState,
    line: &str,
    ifs: &str,
    var_names: &[&str],
) -> Result<(), RustBashError> {
    if ifs.is_empty() || var_names.len() <= 1 {
        // Single variable: assign whole line
        // For REPLY (no named vars), don't trim leading/trailing whitespace
        // For a single named var, only trim IFS whitespace from edges
        let value = if var_names.first().copied() == Some("REPLY") && var_names.len() == 1 {
            // REPLY: strip trailing newline but preserve other whitespace
            line.to_string()
        } else if ifs.is_empty() {
            line.to_string()
        } else {
            let ifs_ws = |c: char| (c == ' ' || c == '\t' || c == '\n') && ifs.contains(c);
            line.trim_matches(ifs_ws).to_string()
        };
        let var_name = var_names.first().copied().unwrap_or("REPLY");
        return set_variable(state, var_name, value);
    }

    // Multiple variables: extract fields one at a time, preserving original text for the last
    let ifs_is_ws = |c: char| (c == ' ' || c == '\t' || c == '\n') && ifs.contains(c);
    let ifs_is_delim = |c: char| ifs.contains(c);
    let has_ws = ifs.contains(' ') || ifs.contains('\t') || ifs.contains('\n');

    let mut pos = 0;
    // Skip leading IFS whitespace
    if has_ws {
        while pos < line.len() {
            let ch = line[pos..].chars().next().unwrap();
            if ifs_is_ws(ch) {
                pos += ch.len_utf8();
            } else {
                break;
            }
        }
    }

    for (i, var_name) in var_names.iter().enumerate() {
        if i == var_names.len() - 1 {
            // Last variable: take the rest of the line, trim trailing IFS whitespace
            let rest = &line[pos..];
            let trimmed = if has_ws {
                rest.trim_end_matches(ifs_is_ws)
            } else {
                rest
            };
            set_variable(state, var_name, trimmed.to_string())?;
        } else {
            // Extract one field
            let field_start = pos;
            while pos < line.len() {
                let ch = line[pos..].chars().next().unwrap();
                if ifs_is_delim(ch) {
                    break;
                }
                pos += ch.len_utf8();
            }
            let field = &line[field_start..pos];
            set_variable(state, var_name, field.to_string())?;

            // Skip separators after the field
            if has_ws {
                while pos < line.len() {
                    let ch = line[pos..].chars().next().unwrap();
                    if ifs_is_ws(ch) {
                        pos += ch.len_utf8();
                    } else {
                        break;
                    }
                }
            }
            // Skip exactly one non-whitespace IFS delimiter if present
            if pos < line.len() {
                let ch = line[pos..].chars().next().unwrap();
                if ifs_is_delim(ch) && !ifs_is_ws(ch) {
                    pos += ch.len_utf8();
                    // Skip trailing IFS whitespace after non-ws delimiter
                    if has_ws {
                        while pos < line.len() {
                            let ch2 = line[pos..].chars().next().unwrap();
                            if ifs_is_ws(ch2) {
                                pos += ch2.len_utf8();
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
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

// ── shopt ───────────────────────────────────────────────────────────

/// Ordered list of all shopt option names (for consistent listing).
const SHOPT_OPTIONS: &[&str] = &[
    "autocd",
    "cdspell",
    "checkhash",
    "checkjobs",
    "checkwinsize",
    "cmdhist",
    "complete_fullquote",
    "direxpand",
    "dirspell",
    "dotglob",
    "execfail",
    "expand_aliases",
    "extglob",
    "extquote",
    "failglob",
    "force_fignore",
    "globasciiranges",
    "globskipdots",
    "globstar",
    "gnu_errfmt",
    "histappend",
    "histreedit",
    "histverify",
    "hostcomplete",
    "huponexit",
    "inherit_errexit",
    "interactive_comments",
    "lastpipe",
    "lithist",
    "login_shell",
    "mailwarn",
    "no_empty_cmd_completion",
    "nocaseglob",
    "nocasematch",
    "nullglob",
    "progcomp",
    "progcomp_alias",
    "promptvars",
    "shift_verbose",
    "sourcepath",
    "xpg_echo",
];

fn get_shopt(state: &InterpreterState, name: &str) -> Option<bool> {
    let o = &state.shopt_opts;
    match name {
        "autocd" => Some(o.autocd),
        "cdspell" => Some(o.cdspell),
        "checkhash" => Some(o.checkhash),
        "checkjobs" => Some(o.checkjobs),
        "checkwinsize" => Some(o.checkwinsize),
        "cmdhist" => Some(o.cmdhist),
        "complete_fullquote" => Some(o.complete_fullquote),
        "direxpand" => Some(o.direxpand),
        "dirspell" => Some(o.dirspell),
        "dotglob" => Some(o.dotglob),
        "execfail" => Some(o.execfail),
        "expand_aliases" => Some(o.expand_aliases),
        "extglob" => Some(o.extglob),
        "extquote" => Some(o.extquote),
        "failglob" => Some(o.failglob),
        "force_fignore" => Some(o.force_fignore),
        "globasciiranges" => Some(o.globasciiranges),
        "globskipdots" => Some(o.globskipdots),
        "globstar" => Some(o.globstar),
        "gnu_errfmt" => Some(o.gnu_errfmt),
        "histappend" => Some(o.histappend),
        "histreedit" => Some(o.histreedit),
        "histverify" => Some(o.histverify),
        "hostcomplete" => Some(o.hostcomplete),
        "huponexit" => Some(o.huponexit),
        "inherit_errexit" => Some(o.inherit_errexit),
        "interactive_comments" => Some(o.interactive_comments),
        "lastpipe" => Some(o.lastpipe),
        "lithist" => Some(o.lithist),
        "login_shell" => Some(o.login_shell),
        "mailwarn" => Some(o.mailwarn),
        "no_empty_cmd_completion" => Some(o.no_empty_cmd_completion),
        "nocaseglob" => Some(o.nocaseglob),
        "nocasematch" => Some(o.nocasematch),
        "nullglob" => Some(o.nullglob),
        "progcomp" => Some(o.progcomp),
        "progcomp_alias" => Some(o.progcomp_alias),
        "promptvars" => Some(o.promptvars),
        "shift_verbose" => Some(o.shift_verbose),
        "sourcepath" => Some(o.sourcepath),
        "xpg_echo" => Some(o.xpg_echo),
        _ => None,
    }
}

fn set_shopt(state: &mut InterpreterState, name: &str, value: bool) -> bool {
    let o = &mut state.shopt_opts;
    match name {
        "autocd" => o.autocd = value,
        "cdspell" => o.cdspell = value,
        "checkhash" => o.checkhash = value,
        "checkjobs" => o.checkjobs = value,
        "checkwinsize" => o.checkwinsize = value,
        "cmdhist" => o.cmdhist = value,
        "complete_fullquote" => o.complete_fullquote = value,
        "direxpand" => o.direxpand = value,
        "dirspell" => o.dirspell = value,
        "dotglob" => o.dotglob = value,
        "execfail" => o.execfail = value,
        "expand_aliases" => o.expand_aliases = value,
        "extglob" => o.extglob = value,
        "extquote" => o.extquote = value,
        "failglob" => o.failglob = value,
        "force_fignore" => o.force_fignore = value,
        "globasciiranges" => o.globasciiranges = value,
        "globskipdots" => o.globskipdots = value,
        "globstar" => o.globstar = value,
        "gnu_errfmt" => o.gnu_errfmt = value,
        "histappend" => o.histappend = value,
        "histreedit" => o.histreedit = value,
        "histverify" => o.histverify = value,
        "hostcomplete" => o.hostcomplete = value,
        "huponexit" => o.huponexit = value,
        "inherit_errexit" => o.inherit_errexit = value,
        "interactive_comments" => o.interactive_comments = value,
        "lastpipe" => o.lastpipe = value,
        "lithist" => o.lithist = value,
        "login_shell" => o.login_shell = value,
        "mailwarn" => o.mailwarn = value,
        "no_empty_cmd_completion" => o.no_empty_cmd_completion = value,
        "nocaseglob" => o.nocaseglob = value,
        "nocasematch" => o.nocasematch = value,
        "nullglob" => o.nullglob = value,
        "progcomp" => o.progcomp = value,
        "progcomp_alias" => o.progcomp_alias = value,
        "promptvars" => o.promptvars = value,
        "shift_verbose" => o.shift_verbose = value,
        "sourcepath" => o.sourcepath = value,
        "xpg_echo" => o.xpg_echo = value,
        _ => return false,
    }
    true
}

fn builtin_shopt(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    // Parse flags
    let mut set_flag = false; // -s
    let mut unset_flag = false; // -u
    let mut query_flag = false; // -q
    let mut print_flag = false; // -p
    let mut o_flag = false; // -o (use set -o options instead of shopt options)
    let mut opt_names: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with('-') && arg.len() > 1 && opt_names.is_empty() {
            for c in arg[1..].chars() {
                match c {
                    's' => set_flag = true,
                    'u' => unset_flag = true,
                    'q' => query_flag = true,
                    'p' => print_flag = true,
                    'o' => o_flag = true,
                    _ => {
                        return Ok(ExecResult {
                            stderr: format!("shopt: -{c}: invalid option\n"),
                            exit_code: 2,
                            ..ExecResult::default()
                        });
                    }
                }
            }
        } else {
            opt_names.push(arg);
        }
        i += 1;
    }

    // If -o flag is set, operate on set -o options instead of shopt options
    if o_flag {
        return shopt_o_mode(
            set_flag, unset_flag, query_flag, print_flag, &opt_names, state,
        );
    }

    // shopt -s opt ... — enable; or shopt -s with no args — list enabled
    if set_flag {
        if opt_names.is_empty() {
            let mut out = String::new();
            for name in SHOPT_OPTIONS {
                if get_shopt(state, name) == Some(true) {
                    out.push_str(&format!("{name:<20}on\n"));
                }
            }
            return Ok(ExecResult {
                stdout: out,
                ..ExecResult::default()
            });
        }
        for name in &opt_names {
            if !set_shopt(state, name, true) {
                return Ok(ExecResult {
                    stderr: format!("shopt: {name}: invalid shell option name\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
        }
        return Ok(ExecResult::default());
    }

    // shopt -u opt ... — disable; or shopt -u with no args — list disabled
    if unset_flag {
        if opt_names.is_empty() {
            let mut out = String::new();
            for name in SHOPT_OPTIONS {
                if get_shopt(state, name) == Some(false) {
                    out.push_str(&format!("{name:<20}off\n"));
                }
            }
            return Ok(ExecResult {
                stdout: out,
                ..ExecResult::default()
            });
        }
        let exit_code = 0;
        for name in &opt_names {
            if !set_shopt(state, name, false) {
                return Ok(ExecResult {
                    stderr: format!("shopt: {name}: invalid shell option name\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
        }
        return Ok(ExecResult {
            exit_code,
            ..ExecResult::default()
        });
    }

    // shopt -q opt ... — query
    if query_flag {
        for name in &opt_names {
            match get_shopt(state, name) {
                Some(true) => {}
                Some(false) => {
                    return Ok(ExecResult {
                        exit_code: 1,
                        ..ExecResult::default()
                    });
                }
                None => {
                    return Ok(ExecResult {
                        stderr: format!("shopt: {name}: invalid shell option name\n"),
                        exit_code: 2,
                        ..ExecResult::default()
                    });
                }
            }
        }
        return Ok(ExecResult::default());
    }

    // shopt -p [opt ...] or shopt with no flags — listing mode
    if print_flag || (!set_flag && !unset_flag && !query_flag) {
        let no_args = opt_names.is_empty();
        let names: Vec<&str> = if no_args {
            SHOPT_OPTIONS.to_vec()
        } else {
            opt_names
        };

        // No-flags, no-args listing: show name on/off format
        if !print_flag && no_args {
            let mut out = String::new();
            for name in SHOPT_OPTIONS {
                let val = get_shopt(state, name).unwrap_or(false);
                let status = if val { "on" } else { "off" };
                out.push_str(&format!("{name:<20}{status}\n"));
            }
            return Ok(ExecResult {
                stdout: out,
                ..ExecResult::default()
            });
        }

        // -p format or named queries without flags
        let mut out = String::new();
        let mut any_invalid = false;
        for name in &names {
            match get_shopt(state, name) {
                Some(val) => {
                    let flag = if val { "-s" } else { "-u" };
                    out.push_str(&format!("shopt {flag} {name}\n"));
                }
                None => {
                    if print_flag {
                        return Ok(ExecResult {
                            stderr: format!("shopt: {name}: invalid shell option name\n"),
                            exit_code: 1,
                            ..ExecResult::default()
                        });
                    }
                    any_invalid = true;
                }
            }
        }
        return Ok(ExecResult {
            stdout: out,
            exit_code: if any_invalid { 1 } else { 0 },
            ..ExecResult::default()
        });
    }

    Ok(ExecResult::default())
}

// ── shopt -o helper ─────────────────────────────────────────────────

const SET_O_OPTIONS: &[&str] = &[
    "allexport",
    "braceexpand",
    "emacs",
    "errexit",
    "hashall",
    "histexpand",
    "history",
    "interactive-comments",
    "monitor",
    "noclobber",
    "noexec",
    "noglob",
    "nounset",
    "pipefail",
    "posix",
    "verbose",
    "vi",
    "xtrace",
];

fn get_set_option(name: &str, state: &InterpreterState) -> Option<bool> {
    match name {
        "allexport" => Some(state.shell_opts.allexport),
        "braceexpand" => Some(true), // always on
        "emacs" => Some(state.shell_opts.emacs_mode),
        "errexit" => Some(state.shell_opts.errexit),
        "hashall" => Some(true), // always on
        "histexpand" => Some(false),
        "history" => Some(false),
        "interactive-comments" => Some(true),
        "monitor" => Some(false),
        "noclobber" => Some(state.shell_opts.noclobber),
        "noexec" => Some(state.shell_opts.noexec),
        "noglob" => Some(state.shell_opts.noglob),
        "nounset" => Some(state.shell_opts.nounset),
        "pipefail" => Some(state.shell_opts.pipefail),
        "posix" => Some(state.shell_opts.posix),
        "verbose" => Some(state.shell_opts.verbose),
        "vi" => Some(state.shell_opts.vi_mode),
        "xtrace" => Some(state.shell_opts.xtrace),
        _ => None,
    }
}

fn shopt_o_mode(
    set_flag: bool,
    unset_flag: bool,
    query_flag: bool,
    print_flag: bool,
    opt_names: &[&str],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    // shopt -o -s opt ... — enable set option
    if set_flag {
        if opt_names.is_empty() {
            let mut out = String::new();
            for name in SET_O_OPTIONS {
                if get_set_option(name, state) == Some(true) {
                    out.push_str(&format!("{name:<20}on\n"));
                }
            }
            return Ok(ExecResult {
                stdout: out,
                ..ExecResult::default()
            });
        }
        for name in opt_names {
            if get_set_option(name, state).is_none() {
                return Ok(ExecResult {
                    stderr: format!("shopt: {name}: invalid shell option name\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
            apply_option_name(name, true, state);
        }
        return Ok(ExecResult::default());
    }

    // shopt -o -u opt ... — disable set option
    if unset_flag {
        if opt_names.is_empty() {
            let mut out = String::new();
            for name in SET_O_OPTIONS {
                if get_set_option(name, state) == Some(false) {
                    out.push_str(&format!("{name:<20}off\n"));
                }
            }
            return Ok(ExecResult {
                stdout: out,
                ..ExecResult::default()
            });
        }
        for name in opt_names {
            if get_set_option(name, state).is_none() {
                return Ok(ExecResult {
                    stderr: format!("shopt: {name}: invalid shell option name\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
            apply_option_name(name, false, state);
        }
        return Ok(ExecResult::default());
    }

    // shopt -o -q opt ... — query
    if query_flag {
        for name in opt_names {
            match get_set_option(name, state) {
                Some(true) => {}
                Some(false) => {
                    return Ok(ExecResult {
                        exit_code: 1,
                        ..ExecResult::default()
                    });
                }
                None => {
                    return Ok(ExecResult {
                        stderr: format!("shopt: {name}: invalid shell option name\n"),
                        exit_code: 2,
                        ..ExecResult::default()
                    });
                }
            }
        }
        return Ok(ExecResult::default());
    }

    // shopt -p -o / shopt -o (listing)
    let no_args = opt_names.is_empty();
    let names: Vec<&str> = if no_args {
        SET_O_OPTIONS.to_vec()
    } else {
        opt_names.to_vec()
    };

    if !print_flag && no_args {
        let mut out = String::new();
        for name in SET_O_OPTIONS {
            let val = get_set_option(name, state).unwrap_or(false);
            let status = if val { "on" } else { "off" };
            out.push_str(&format!("{name:<20}{status}\n"));
        }
        return Ok(ExecResult {
            stdout: out,
            ..ExecResult::default()
        });
    }

    let mut out = String::new();
    let mut any_invalid = false;
    for name in &names {
        match get_set_option(name, state) {
            Some(val) => {
                let flag = if val { "-o" } else { "+o" };
                out.push_str(&format!("set {flag} {name}\n"));
            }
            None => {
                if print_flag {
                    return Ok(ExecResult {
                        stderr: format!("shopt: {name}: invalid shell option name\n"),
                        exit_code: 1,
                        ..ExecResult::default()
                    });
                }
                any_invalid = true;
            }
        }
    }
    Ok(ExecResult {
        stdout: out,
        exit_code: if any_invalid { 1 } else { 0 },
        ..ExecResult::default()
    })
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
    // Parse flags similar to declare
    let mut make_indexed_array = false;
    let mut make_assoc_array = false;
    let mut make_readonly = false;
    let mut make_exported = false;
    let mut make_integer = false;
    let mut make_nameref = false;
    let mut var_args: Vec<&String> = Vec::new();

    for arg in args {
        if let Some(flags) = arg.strip_prefix('-') {
            if flags.is_empty() {
                var_args.push(arg);
                continue;
            }
            for c in flags.chars() {
                match c {
                    'a' => make_indexed_array = true,
                    'A' => make_assoc_array = true,
                    'r' => make_readonly = true,
                    'x' => make_exported = true,
                    'i' => make_integer = true,
                    'n' => make_nameref = true,
                    _ => {}
                }
            }
        } else {
            var_args.push(arg);
        }
    }

    for arg in &var_args {
        if let Some((raw_name, value)) = arg.split_once("+=") {
            // local name+=value — append
            let name = raw_name;
            if let Some(scope) = state.local_scopes.last_mut() {
                scope
                    .entry(name.to_string())
                    .or_insert_with(|| state.env.get(name).cloned());
            }
            if value.starts_with('(') && value.ends_with(')') {
                // Array append
                let inner = &value[1..value.len() - 1];
                let start_idx = match state.env.get(name) {
                    Some(var) => match &var.value {
                        VariableValue::IndexedArray(map) => {
                            map.keys().next_back().map(|k| k + 1).unwrap_or(0)
                        }
                        VariableValue::Scalar(s) if s.is_empty() => 0,
                        VariableValue::Scalar(_) => 1,
                        _ => 0,
                    },
                    None => 0,
                };
                if !state.env.contains_key(name) {
                    state.env.insert(
                        name.to_string(),
                        Variable {
                            value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                            attrs: VariableAttrs::empty(),
                        },
                    );
                }
                let words = shell_split_array_body(inner);
                let mut idx = start_idx;
                for word in &words {
                    let val = unquote_simple(word);
                    crate::interpreter::set_array_element(state, name, idx, val)?;
                    idx += 1;
                }
            } else {
                let current = state
                    .env
                    .get(name)
                    .map(|v| v.value.as_scalar().to_string())
                    .unwrap_or_default();
                let new_val = format!("{current}{value}");
                set_variable(state, name, new_val)?;
            }
        } else if let Some((name, value)) = arg.split_once('=') {
            // Save current value in the top local scope (if inside a function)
            if let Some(scope) = state.local_scopes.last_mut() {
                scope
                    .entry(name.to_string())
                    .or_insert_with(|| state.env.get(name).cloned());
            }

            if make_assoc_array {
                if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
                    state.env.insert(
                        name.to_string(),
                        Variable {
                            value: VariableValue::AssociativeArray(
                                std::collections::BTreeMap::new(),
                            ),
                            attrs: VariableAttrs::empty(),
                        },
                    );
                    // Parse associative array body
                    let words = shell_split_array_body(inner);
                    for word in &words {
                        if let Some(rest) = word.strip_prefix('[')
                            && let Some(eq_pos) = rest.find("]=")
                        {
                            let key = &rest[..eq_pos];
                            let val = unquote_simple(&rest[eq_pos + 2..]);
                            if let Some(var) = state.env.get_mut(name)
                                && let VariableValue::AssociativeArray(map) = &mut var.value
                            {
                                map.insert(key.to_string(), val);
                            }
                        }
                    }
                } else {
                    set_variable(state, name, value.to_string())?;
                }
            } else if make_indexed_array || value.starts_with('(') && value.ends_with(')') {
                if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
                    state.env.insert(
                        name.to_string(),
                        Variable {
                            value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                            attrs: VariableAttrs::empty(),
                        },
                    );
                    parse_and_set_indexed_array(state, name, inner)?;
                } else {
                    set_variable(state, name, value.to_string())?;
                }
            } else {
                set_variable(state, name, value.to_string())?;
            }

            // Apply attribute flags
            if let Some(var) = state.env.get_mut(name) {
                if make_readonly {
                    var.attrs.insert(VariableAttrs::READONLY);
                }
                if make_exported {
                    var.attrs.insert(VariableAttrs::EXPORTED);
                }
                if make_integer {
                    var.attrs.insert(VariableAttrs::INTEGER);
                }
                if make_nameref {
                    var.attrs.insert(VariableAttrs::NAMEREF);
                }
            }
        } else {
            // `local VAR` with no value — declare it as local with empty value
            if let Some(scope) = state.local_scopes.last_mut() {
                scope
                    .entry(arg.to_string())
                    .or_insert_with(|| state.env.get(arg.as_str()).cloned());
            }
            // Inside a function: always set to empty. Outside: only if undefined.
            if state.in_function_depth > 0 || !state.env.contains_key(arg.as_str()) {
                let value = if make_indexed_array {
                    VariableValue::IndexedArray(std::collections::BTreeMap::new())
                } else if make_assoc_array {
                    VariableValue::AssociativeArray(std::collections::BTreeMap::new())
                } else {
                    VariableValue::Scalar(String::new())
                };
                state.env.insert(
                    arg.to_string(),
                    Variable {
                        value,
                        attrs: VariableAttrs::empty(),
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

// ── PATH resolution helper ─────────────────────────────────────────

/// Search `$PATH` directories in the VFS for an executable file.
fn search_path(cmd: &str, state: &InterpreterState) -> Option<String> {
    let path_var = state
        .env
        .get("PATH")
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_else(|| "/usr/bin:/bin".to_string());

    for dir in path_var.split(':') {
        let candidate = if dir.is_empty() {
            format!("./{cmd}")
        } else {
            format!("{dir}/{cmd}")
        };
        let p = Path::new(&candidate);
        if state.fs.exists(p)
            && let Ok(meta) = state.fs.stat(p)
            && matches!(meta.node_type, NodeType::File)
        {
            return Some(candidate);
        }
    }
    None
}

// ── type ────────────────────────────────────────────────────────────

fn builtin_type(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let mut t_flag = false;
    let mut a_flag = false;
    let mut p_flag = false;
    let mut names: Vec<&str> = Vec::new();

    for arg in args {
        if arg.starts_with('-') && names.is_empty() {
            for c in arg[1..].chars() {
                match c {
                    't' => t_flag = true,
                    'a' => a_flag = true,
                    'p' => p_flag = true,
                    _ => {
                        return Ok(ExecResult {
                            stderr: format!("type: -{c}: invalid option\n"),
                            exit_code: 2,
                            ..ExecResult::default()
                        });
                    }
                }
            }
        } else {
            names.push(arg);
        }
    }

    if names.is_empty() {
        return Ok(ExecResult::default());
    }

    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0;

    for name in &names {
        let mut found = false;

        // Check alias
        if let Some(expansion) = state.aliases.get(*name) {
            if t_flag {
                stdout.push_str("alias\n");
            } else if !p_flag {
                stdout.push_str(&format!("{name} is aliased to `{expansion}'\n"));
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        // Check function
        if state.functions.contains_key(*name) {
            if t_flag {
                stdout.push_str("function\n");
            } else if !p_flag {
                stdout.push_str(&format!("{name} is a function\n"));
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        // Check builtin
        if is_builtin(name) {
            if t_flag {
                stdout.push_str("builtin\n");
            } else if !p_flag && !t_flag {
                stdout.push_str(&format!("{name} is a shell builtin\n"));
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        // Check registered commands (treated as builtins)
        if !is_builtin(name) && state.commands.contains_key(*name) {
            if t_flag {
                stdout.push_str("builtin\n");
            } else if !p_flag && !t_flag {
                stdout.push_str(&format!("{name} is a shell builtin\n"));
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        // Check PATH
        if let Some(path) = search_path(name, state) {
            if t_flag {
                stdout.push_str("file\n");
            } else if p_flag {
                stdout.push_str(&format!("{path}\n"));
            } else {
                stdout.push_str(&format!("{name} is {path}\n"));
            }
            found = true;
        }

        if !found {
            stderr.push_str(&format!("type: {name}: not found\n"));
            exit_code = 1;
        }
    }

    Ok(ExecResult {
        stdout,
        stderr,
        exit_code,
        stdout_bytes: None,
    })
}

// ── command ─────────────────────────────────────────────────────────

fn builtin_command(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let mut v_flag = false;
    let mut big_v_flag = false;
    let mut cmd_start = 0;

    // Parse flags
    for (i, arg) in args.iter().enumerate() {
        if arg.starts_with('-') && cmd_start == i {
            let mut consumed = true;
            for c in arg[1..].chars() {
                match c {
                    'v' => v_flag = true,
                    'V' => big_v_flag = true,
                    'p' => { /* use default PATH — we ignore this in sandbox */ }
                    _ => {
                        consumed = false;
                        break;
                    }
                }
            }
            if consumed {
                cmd_start = i + 1;
            } else {
                break;
            }
        } else {
            break;
        }
    }

    let remaining = &args[cmd_start..];
    if remaining.is_empty() {
        return Ok(ExecResult::default());
    }

    let name = &remaining[0];

    // command -v: print how name would be resolved
    if v_flag {
        return command_v(name, state);
    }

    // command -V: verbose description
    if big_v_flag {
        return command_big_v(name, state);
    }

    // command name [args]: run bypassing functions — only builtins and commands
    let cmd_args = &remaining[1..];
    let cmd_args_owned: Vec<String> = cmd_args.to_vec();

    // --help interception (consistent with dispatch_command)
    if cmd_args_owned.first().map(|a| a.as_str()) == Some("--help")
        && let Some(help) = check_help(name, state)
    {
        return Ok(help);
    }

    // Try builtin first
    if let Some(result) = execute_builtin(name, &cmd_args_owned, state, stdin)? {
        return Ok(result);
    }

    // Try registered commands (skip functions)
    if state.commands.contains_key(name.as_str()) {
        // Re-dispatch through the normal path but the caller should skip functions.
        // We replicate the command execution logic here.
        let env: std::collections::HashMap<String, String> = state
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.value.as_scalar().to_string()))
            .collect();
        let fs = std::sync::Arc::clone(&state.fs);
        let cwd = state.cwd.clone();
        let limits = state.limits.clone();
        let network_policy = state.network_policy.clone();

        let ctx = crate::commands::CommandContext {
            fs: &*fs,
            cwd: &cwd,
            env: &env,
            variables: None,
            stdin,
            stdin_bytes: None,
            limits: &limits,
            network_policy: &network_policy,
            exec: None,
        };

        let cmd = state.commands.get(name.as_str()).unwrap();
        let cmd_result = cmd.execute(&cmd_args_owned, &ctx);
        return Ok(ExecResult {
            stdout: cmd_result.stdout,
            stderr: cmd_result.stderr,
            exit_code: cmd_result.exit_code,
            stdout_bytes: cmd_result.stdout_bytes,
        });
    }

    // Not found
    Ok(ExecResult {
        stderr: format!("{name}: command not found\n"),
        exit_code: 127,
        ..ExecResult::default()
    })
}

fn command_v(name: &str, state: &InterpreterState) -> Result<ExecResult, RustBashError> {
    // Alias
    if let Some(expansion) = state.aliases.get(name) {
        return Ok(ExecResult {
            stdout: format!("alias {name}='{expansion}'\n"),
            ..ExecResult::default()
        });
    }

    // Function
    if state.functions.contains_key(name) {
        return Ok(ExecResult {
            stdout: format!("{name}\n"),
            ..ExecResult::default()
        });
    }

    // Builtin or registered command
    if is_builtin(name) || state.commands.contains_key(name) {
        return Ok(ExecResult {
            stdout: format!("{name}\n"),
            ..ExecResult::default()
        });
    }

    // PATH search
    if let Some(path) = search_path(name, state) {
        return Ok(ExecResult {
            stdout: format!("{path}\n"),
            ..ExecResult::default()
        });
    }

    Ok(ExecResult {
        exit_code: 1,
        ..ExecResult::default()
    })
}

fn command_big_v(name: &str, state: &InterpreterState) -> Result<ExecResult, RustBashError> {
    if let Some(expansion) = state.aliases.get(name) {
        return Ok(ExecResult {
            stdout: format!("{name} is aliased to `{expansion}'\n"),
            ..ExecResult::default()
        });
    }

    if state.functions.contains_key(name) {
        return Ok(ExecResult {
            stdout: format!("{name} is a function\n"),
            ..ExecResult::default()
        });
    }

    if is_builtin(name) || state.commands.contains_key(name) {
        return Ok(ExecResult {
            stdout: format!("{name} is a shell builtin\n"),
            ..ExecResult::default()
        });
    }

    if let Some(path) = search_path(name, state) {
        return Ok(ExecResult {
            stdout: format!("{name} is {path}\n"),
            ..ExecResult::default()
        });
    }

    Ok(ExecResult {
        stderr: format!("command: {name}: not found\n"),
        exit_code: 1,
        ..ExecResult::default()
    })
}

// ── builtin (the keyword) ──────────────────────────────────────────

fn builtin_builtin(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        return Ok(ExecResult::default());
    }

    let name = &args[0];
    let sub_args: Vec<String> = args[1..].to_vec();

    // --help interception (consistent with dispatch_command)
    if sub_args.first().map(|a| a.as_str()) == Some("--help")
        && let Some(help) = check_help(name, state)
    {
        return Ok(help);
    }

    // Try shell builtins first
    if let Some(result) = execute_builtin(name, &sub_args, state, stdin)? {
        return Ok(result);
    }

    // Also try registered commands (echo, printf, etc. are implemented as commands)
    if let Some(cmd) = state.commands.get(name.as_str()) {
        let env: std::collections::HashMap<String, String> = state
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.value.as_scalar().to_string()))
            .collect();
        let fs = std::sync::Arc::clone(&state.fs);
        let cwd = state.cwd.clone();
        let limits = state.limits.clone();
        let network_policy = state.network_policy.clone();

        let ctx = crate::commands::CommandContext {
            fs: &*fs,
            cwd: &cwd,
            env: &env,
            variables: None,
            stdin,
            stdin_bytes: None,
            limits: &limits,
            network_policy: &network_policy,
            exec: None,
        };

        let cmd_result = cmd.execute(&sub_args, &ctx);
        return Ok(ExecResult {
            stdout: cmd_result.stdout,
            stderr: cmd_result.stderr,
            exit_code: cmd_result.exit_code,
            stdout_bytes: cmd_result.stdout_bytes,
        });
    }

    Ok(ExecResult {
        stderr: format!("builtin: {name}: not a shell builtin\n"),
        exit_code: 1,
        ..ExecResult::default()
    })
}

// ── getopts ─────────────────────────────────────────────────────────

fn builtin_getopts(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.len() < 2 {
        return Ok(ExecResult {
            stderr: "getopts: usage: getopts optstring name [arg ...]\n".to_string(),
            exit_code: 2,
            ..ExecResult::default()
        });
    }

    let optstring = &args[0];
    let var_name = &args[1];

    // If extra args provided, use them; otherwise use positional params
    let option_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        state.positional_params.clone()
    };

    // Loop instead of recursion: advance to the next argument when the
    // sub-position within bundled flags has been exhausted.
    loop {
        let optind: usize = state
            .env
            .get("OPTIND")
            .and_then(|v| v.value.as_scalar().parse().ok())
            .unwrap_or(1);

        let idx = optind.saturating_sub(1);

        if idx >= option_args.len() {
            set_variable(state, var_name, "?".to_string())?;
            return Ok(ExecResult {
                exit_code: 1,
                ..ExecResult::default()
            });
        }

        let current_arg = &option_args[idx];

        if !current_arg.starts_with('-') || current_arg == "-" || current_arg == "--" {
            set_variable(state, var_name, "?".to_string())?;
            if current_arg == "--" {
                set_variable(state, "OPTIND", (optind + 1).to_string())?;
            }
            return Ok(ExecResult {
                exit_code: 1,
                ..ExecResult::default()
            });
        }

        let opt_chars: Vec<char> = current_arg[1..].chars().collect();

        let sub_pos: usize = state
            .env
            .get("__GETOPTS_SUBPOS")
            .and_then(|v| v.value.as_scalar().parse().ok())
            .unwrap_or(0);

        if sub_pos >= opt_chars.len() {
            // Advance to next argument and retry (loop, not recurse).
            set_variable(state, "__GETOPTS_SUBPOS", "0".to_string())?;
            set_variable(state, "OPTIND", (optind + 1).to_string())?;
            continue;
        }

        let opt_char = opt_chars[sub_pos];
        let silent = optstring.starts_with(':');
        let optstring_chars: &str = if silent { &optstring[1..] } else { optstring };
        let opt_pos = optstring_chars.find(opt_char);

        if let Some(pos) = opt_pos {
            let takes_arg = optstring_chars.chars().nth(pos + 1) == Some(':');

            if takes_arg {
                let rest: String = opt_chars[sub_pos + 1..].iter().collect();
                if !rest.is_empty() {
                    set_variable(state, "OPTARG", rest)?;
                    set_variable(state, "__GETOPTS_SUBPOS", "0".to_string())?;
                    set_variable(state, "OPTIND", (optind + 1).to_string())?;
                } else if idx + 1 < option_args.len() {
                    set_variable(state, "OPTARG", option_args[idx + 1].clone())?;
                    set_variable(state, "__GETOPTS_SUBPOS", "0".to_string())?;
                    set_variable(state, "OPTIND", (optind + 2).to_string())?;
                } else {
                    // Missing argument
                    set_variable(state, "__GETOPTS_SUBPOS", "0".to_string())?;
                    set_variable(state, "OPTIND", (optind + 1).to_string())?;
                    if silent {
                        set_variable(state, var_name, ":".to_string())?;
                        set_variable(state, "OPTARG", opt_char.to_string())?;
                        return Ok(ExecResult::default());
                    }
                    set_variable(state, var_name, "?".to_string())?;
                    return Ok(ExecResult {
                        stderr: format!("getopts: option requires an argument -- '{opt_char}'\n"),
                        ..ExecResult::default()
                    });
                }
            } else {
                state.env.remove("OPTARG");
                if sub_pos + 1 < opt_chars.len() {
                    set_variable(state, "__GETOPTS_SUBPOS", (sub_pos + 1).to_string())?;
                } else {
                    set_variable(state, "__GETOPTS_SUBPOS", "0".to_string())?;
                    set_variable(state, "OPTIND", (optind + 1).to_string())?;
                }
            }
            set_variable(state, var_name, opt_char.to_string())?;
            return Ok(ExecResult::default());
        }

        // Invalid option
        if silent {
            set_variable(state, var_name, "?".to_string())?;
            set_variable(state, "OPTARG", opt_char.to_string())?;
        } else {
            set_variable(state, var_name, "?".to_string())?;
        }
        if sub_pos + 1 < opt_chars.len() {
            set_variable(state, "__GETOPTS_SUBPOS", (sub_pos + 1).to_string())?;
        } else {
            set_variable(state, "__GETOPTS_SUBPOS", "0".to_string())?;
            set_variable(state, "OPTIND", (optind + 1).to_string())?;
        }
        let stderr = if silent {
            String::new()
        } else {
            format!("getopts: illegal option -- '{opt_char}'\n")
        };
        return Ok(ExecResult {
            stderr,
            ..ExecResult::default()
        });
    }
}

// ── mapfile / readarray ─────────────────────────────────────────────

fn builtin_mapfile(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    let mut strip_newline = false;
    let mut delimiter = '\n';
    let mut max_count: Option<usize> = None;
    let mut skip_count: usize = 0;
    let mut array_name = "MAPFILE".to_string();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        if arg.starts_with('-') && arg.len() > 1 {
            let mut chars = arg[1..].chars();
            while let Some(c) = chars.next() {
                match c {
                    't' => strip_newline = true,
                    'd' => {
                        let rest: String = chars.collect();
                        let delim_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() { args[i].as_str() } else { "" }
                        } else {
                            &rest
                        };
                        delimiter = delim_str.chars().next().unwrap_or('\0');
                        break;
                    }
                    'n' => {
                        let rest: String = chars.collect();
                        let count_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() {
                                args[i].as_str()
                            } else {
                                "0"
                            }
                        } else {
                            &rest
                        };
                        max_count = count_str.parse().ok();
                        break;
                    }
                    's' => {
                        let rest: String = chars.collect();
                        let count_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() {
                                args[i].as_str()
                            } else {
                                "0"
                            }
                        } else {
                            &rest
                        };
                        skip_count = count_str.parse().unwrap_or(0);
                        break;
                    }
                    'C' | 'c' | 'O' | 'u' => {
                        // -C callback, -c quantum, -O origin, -u fd — skip values
                        let rest: String = chars.collect();
                        if rest.is_empty() {
                            i += 1; // skip the argument value
                        }
                        break;
                    }
                    _ => {
                        return Ok(ExecResult {
                            stderr: format!("mapfile: -{c}: invalid option\n"),
                            exit_code: 2,
                            ..ExecResult::default()
                        });
                    }
                }
            }
        } else {
            array_name = arg.clone();
        }
        i += 1;
    }

    // Split stdin by delimiter
    let lines: Vec<&str> = if delimiter == '\0' {
        // NUL delimiter: split on NUL
        stdin.split('\0').collect()
    } else {
        split_keeping_delimiter(stdin, delimiter)
    };

    // Build the array
    let mut map = std::collections::BTreeMap::new();
    let mut count = 0;

    for (line_idx, line) in lines.iter().enumerate() {
        if line_idx < skip_count {
            continue;
        }
        if let Some(max) = max_count
            && count >= max
        {
            break;
        }

        let value = if strip_newline {
            line.trim_end_matches(delimiter).to_string()
        } else {
            (*line).to_string()
        };

        if map.len() >= state.limits.max_array_elements {
            return Err(RustBashError::LimitExceeded {
                limit_name: "max_array_elements",
                limit_value: state.limits.max_array_elements,
                actual_value: map.len() + 1,
            });
        }
        map.insert(count, value);
        count += 1;
    }

    state.env.insert(
        array_name,
        Variable {
            value: VariableValue::IndexedArray(map),
            attrs: VariableAttrs::empty(),
        },
    );

    Ok(ExecResult::default())
}

/// Split a string by delimiter, keeping the delimiter at the end of each segment
/// (like bash mapfile behavior — each line includes its trailing newline).
fn split_keeping_delimiter(s: &str, delim: char) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c == delim {
            let end = i + c.len_utf8();
            result.push(&s[start..end]);
            start = end;
        }
    }
    // Don't add empty trailing segment
    if start < s.len() {
        result.push(&s[start..]);
    }
    result
}

// ── pushd ───────────────────────────────────────────────────────────

fn builtin_pushd(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // pushd with no args: swap top two
        if state.dir_stack.is_empty() {
            return Ok(ExecResult {
                stderr: "pushd: no other directory\n".to_string(),
                exit_code: 1,
                ..ExecResult::default()
            });
        }
        let top = state.dir_stack.remove(0);
        let old_cwd = state.cwd.clone();
        // cd to top
        let result = builtin_cd(std::slice::from_ref(&top), state)?;
        if result.exit_code != 0 {
            state.dir_stack.insert(0, top);
            return Ok(result);
        }
        state.dir_stack.insert(0, old_cwd);
        return Ok(dirs_output(state));
    }

    let arg = &args[0];

    // pushd +N / -N: rotate stack
    if (arg.starts_with('+') || arg.starts_with('-'))
        && let Ok(n) = arg[1..].parse::<usize>()
    {
        let stack_size = state.dir_stack.len() + 1; // +1 for cwd
        if n >= stack_size {
            return Ok(ExecResult {
                stderr: format!("pushd: {arg}: directory stack index out of range\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
        }

        // Build full stack: cwd + dir_stack
        let mut full_stack = vec![state.cwd.clone()];
        full_stack.extend(state.dir_stack.iter().cloned());

        let rotate_n = if arg.starts_with('+') {
            n
        } else {
            stack_size - n
        };
        full_stack.rotate_left(rotate_n);

        state.cwd = full_stack.remove(0);
        state.dir_stack = full_stack;

        let cwd = state.cwd.clone();
        let _ = set_variable(state, "PWD", cwd);
        return Ok(dirs_output(state));
    }

    // pushd dir: push current dir, cd to dir
    let old_cwd = state.cwd.clone();
    let result = builtin_cd(std::slice::from_ref(arg), state)?;
    if result.exit_code != 0 {
        return Ok(result);
    }
    state.dir_stack.insert(0, old_cwd);

    Ok(dirs_output(state))
}

// ── popd ────────────────────────────────────────────────────────────

fn builtin_popd(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if state.dir_stack.is_empty() {
        return Ok(ExecResult {
            stderr: "popd: directory stack empty\n".to_string(),
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    if !args.is_empty() {
        let arg = &args[0];
        // popd +N / -N: remove Nth entry
        if (arg.starts_with('+') || arg.starts_with('-'))
            && let Ok(n) = arg[1..].parse::<usize>()
        {
            let stack_size = state.dir_stack.len() + 1;
            if n >= stack_size {
                return Ok(ExecResult {
                    stderr: format!("popd: {arg}: directory stack index out of range\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
            let idx = if arg.starts_with('+') {
                n
            } else {
                stack_size - 1 - n
            };
            if idx == 0 {
                // Remove cwd, set cwd to next
                let new_cwd = state.dir_stack.remove(0);
                state.cwd = new_cwd;
                let cwd = state.cwd.clone();
                let _ = set_variable(state, "PWD", cwd);
            } else {
                state.dir_stack.remove(idx - 1);
            }
            return Ok(dirs_output(state));
        }
    }

    // Default: pop top and cd there
    let top = state.dir_stack.remove(0);
    let result = builtin_cd(std::slice::from_ref(&top), state)?;
    if result.exit_code != 0 {
        state.dir_stack.insert(0, top);
        return Ok(result);
    }

    Ok(dirs_output(state))
}

// ── dirs ────────────────────────────────────────────────────────────

fn builtin_dirs(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let mut clear = false;
    let mut per_line = false;
    let mut with_index = false;
    let mut long_format = false;

    for arg in args {
        if let Some(flags) = arg.strip_prefix('-') {
            for c in flags.chars() {
                match c {
                    'c' => clear = true,
                    'p' => per_line = true,
                    'v' => {
                        with_index = true;
                        per_line = true;
                    }
                    'l' => long_format = true,
                    _ => {
                        return Ok(ExecResult {
                            stderr: format!("dirs: -{c}: invalid option\n"),
                            exit_code: 2,
                            ..ExecResult::default()
                        });
                    }
                }
            }
        }
    }

    if clear {
        state.dir_stack.clear();
        return Ok(ExecResult::default());
    }

    let home = state
        .env
        .get("HOME")
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_default();

    // Build stack: cwd at position 0, then dir_stack entries
    let mut entries = vec![state.cwd.clone()];
    entries.extend(state.dir_stack.iter().cloned());

    let mut stdout = String::new();
    if with_index {
        for (i, entry) in entries.iter().enumerate() {
            let display = if !long_format
                && !home.is_empty()
                && (*entry == home || entry.starts_with(&format!("{home}/")))
            {
                format!("~{}", &entry[home.len()..])
            } else {
                entry.clone()
            };
            stdout.push_str(&format!(" {i}\t{display}\n"));
        }
    } else if per_line {
        for entry in &entries {
            let display = if !long_format
                && !home.is_empty()
                && (*entry == home || entry.starts_with(&format!("{home}/")))
            {
                format!("~{}", &entry[home.len()..])
            } else {
                entry.clone()
            };
            stdout.push_str(&format!("{display}\n"));
        }
    } else {
        let display_entries: Vec<String> = entries
            .iter()
            .map(|e| {
                if !long_format
                    && !home.is_empty()
                    && (*e == home || e.starts_with(&format!("{home}/")))
                {
                    format!("~{}", &e[home.len()..])
                } else {
                    e.clone()
                }
            })
            .collect();
        stdout = display_entries.join(" ");
        stdout.push('\n');
    }

    Ok(ExecResult {
        stdout,
        ..ExecResult::default()
    })
}

/// Helper to produce `dirs`-style output for pushd/popd.
fn dirs_output(state: &InterpreterState) -> ExecResult {
    let mut entries = vec![state.cwd.clone()];
    entries.extend(state.dir_stack.iter().cloned());

    let home = state
        .env
        .get("HOME")
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_default();

    let display_entries: Vec<String> = entries
        .iter()
        .map(|e| {
            if !home.is_empty() && (*e == home || e.starts_with(&format!("{home}/"))) {
                format!("~{}", &e[home.len()..])
            } else {
                e.clone()
            }
        })
        .collect();

    ExecResult {
        stdout: format!("{}\n", display_entries.join(" ")),
        ..ExecResult::default()
    }
}

// ── hash ────────────────────────────────────────────────────────────

fn builtin_hash(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // List all hashed commands
        if state.command_hash.is_empty() {
            return Ok(ExecResult {
                stderr: "hash: hash table empty\n".to_string(),
                ..ExecResult::default()
            });
        }
        let mut stdout = String::new();
        let mut entries: Vec<(&String, &String)> = state.command_hash.iter().collect();
        entries.sort_by_key(|(k, _)| k.as_str());
        for (name, path) in entries {
            stdout.push_str(&format!("{name}={path}\n"));
        }
        return Ok(ExecResult {
            stdout,
            ..ExecResult::default()
        });
    }

    let mut reset = false;
    let mut names: Vec<&str> = Vec::new();

    for arg in args {
        if arg == "-r" {
            reset = true;
        } else if arg.starts_with('-') {
            // Other flags like -d, -l, -t: ignore for now
        } else {
            names.push(arg);
        }
    }

    if reset {
        state.command_hash.clear();
    }

    for name in &names {
        if let Some(path) = search_path(name, state) {
            state.command_hash.insert(name.to_string(), path);
        } else {
            return Ok(ExecResult {
                stderr: format!("hash: {name}: not found\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
        }
    }

    Ok(ExecResult::default())
}

// ── alias / unalias ─────────────────────────────────────────────────

fn builtin_alias(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // List all aliases
        let mut entries: Vec<(&String, &String)> = state.aliases.iter().collect();
        entries.sort_by_key(|(k, _)| k.as_str());
        let mut stdout = String::new();
        for (name, value) in entries {
            stdout.push_str(&format!("alias {name}='{value}'\n"));
        }
        return Ok(ExecResult {
            stdout,
            ..ExecResult::default()
        });
    }

    let mut exit_code = 0;
    let mut stdout = String::new();
    let mut stderr = String::new();

    for arg in args {
        if arg.starts_with('-') {
            // -p flag: print all aliases (same as no args)
            if arg == "-p" {
                let mut entries: Vec<(&String, &String)> = state.aliases.iter().collect();
                entries.sort_by_key(|(k, _)| k.as_str());
                for (name, value) in &entries {
                    stdout.push_str(&format!("alias {name}='{value}'\n"));
                }
            }
            continue;
        }

        if let Some(eq_pos) = arg.find('=') {
            // alias name=value
            let name = &arg[..eq_pos];
            let value = &arg[eq_pos + 1..];
            state.aliases.insert(name.to_string(), value.to_string());
        } else {
            // alias name — print this alias
            if let Some(value) = state.aliases.get(arg.as_str()) {
                stdout.push_str(&format!("alias {arg}='{value}'\n"));
            } else {
                stderr.push_str(&format!("alias: {arg}: not found\n"));
                exit_code = 1;
            }
        }
    }

    Ok(ExecResult {
        stdout,
        stderr,
        exit_code,
        stdout_bytes: None,
    })
}

fn builtin_unalias(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        return Ok(ExecResult {
            stderr: "unalias: usage: unalias [-a] name [name ...]\n".to_string(),
            exit_code: 2,
            ..ExecResult::default()
        });
    }

    let mut exit_code = 0;
    let mut stderr = String::new();

    for arg in args {
        if arg == "-a" {
            state.aliases.clear();
            continue;
        }
        if state.aliases.remove(arg.as_str()).is_none() {
            stderr.push_str(&format!("unalias: {arg}: not found\n"));
            exit_code = 1;
        }
    }

    Ok(ExecResult {
        stderr,
        exit_code,
        ..ExecResult::default()
    })
}

// ── printf ───────────────────────────────────────────────────────────

fn builtin_printf(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        return Ok(ExecResult {
            stderr: "printf: usage: printf [-v var] format [arguments]\n".into(),
            exit_code: 2,
            ..ExecResult::default()
        });
    }

    let mut var_name: Option<String> = None;
    let mut remaining_args = args;

    // Parse -v varname
    if remaining_args.len() >= 2 && remaining_args[0] == "-v" {
        var_name = Some(remaining_args[1].clone());
        remaining_args = &remaining_args[2..];
    }

    if remaining_args.is_empty() {
        return Ok(ExecResult {
            stderr: "printf: usage: printf [-v var] format [arguments]\n".into(),
            exit_code: 2,
            ..ExecResult::default()
        });
    }

    let format_str = &remaining_args[0];
    let arguments = &remaining_args[1..];
    let result = crate::commands::text::run_printf_format(format_str, arguments);

    let exit_code = if result.had_error { 1 } else { 0 };

    if let Some(name) = var_name {
        set_variable(state, &name, result.stdout)?;
        Ok(ExecResult {
            stderr: result.stderr,
            exit_code,
            ..ExecResult::default()
        })
    } else {
        Ok(ExecResult {
            stdout: result.stdout,
            stderr: result.stderr,
            exit_code,
            ..ExecResult::default()
        })
    }
}

// ── sh / bash builtin ───────────────────────────────────────────────

fn builtin_sh(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        if stdin.is_empty() {
            return Ok(ExecResult::default());
        }
        let program = parse(stdin)?;
        return run_in_subshell(state, &program, &[], None);
    }

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-c" {
            i += 1;
            if i < args.len() {
                let cmd = &args[i];
                let extra = &args[i + 1..];
                // In bash: sh -c cmd arg0 arg1 → $0=arg0, $1=arg1
                let shell_name_override = extra.first().map(|s| s.as_str());
                let positional: Vec<String> = if extra.len() > 1 {
                    extra[1..].iter().map(|s| s.to_string()).collect()
                } else {
                    Vec::new()
                };
                let program = parse(cmd)?;
                return run_in_subshell(state, &program, &positional, shell_name_override);
            } else {
                return Ok(ExecResult {
                    stderr: "sh: -c: option requires an argument\n".into(),
                    exit_code: 2,
                    ..ExecResult::default()
                });
            }
        } else if arg.starts_with('-') && arg.len() > 1 {
            i += 1;
            continue;
        } else {
            let path = crate::interpreter::builtins::resolve_path(&state.cwd, arg);
            let path_buf = std::path::PathBuf::from(&path);
            match state.fs.read_file(&path_buf) {
                Ok(bytes) => {
                    let script = String::from_utf8_lossy(&bytes).to_string();
                    let positional = args[i + 1..]
                        .iter()
                        .map(|s| s.to_string())
                        .collect::<Vec<_>>();
                    let program = parse(&script)?;
                    return run_in_subshell(state, &program, &positional, None);
                }
                Err(e) => {
                    return Ok(ExecResult {
                        stderr: format!("sh: {}: {}\n", arg, e),
                        exit_code: 127,
                        ..ExecResult::default()
                    });
                }
            }
        }
    }

    Ok(ExecResult::default())
}

/// Execute a parsed program in an isolated subshell, returning only
/// stdout/stderr/exit_code. Mirrors `execute_subshell` in walker.rs.
fn run_in_subshell(
    state: &mut InterpreterState,
    program: &brush_parser::ast::Program,
    positional: &[String],
    shell_name_override: Option<&str>,
) -> Result<ExecResult, RustBashError> {
    use std::collections::HashMap;
    let cloned_fs = state.fs.deep_clone();
    let mut sub_state = InterpreterState {
        fs: cloned_fs,
        env: state.env.clone(),
        cwd: state.cwd.clone(),
        functions: state.functions.clone(),
        last_exit_code: state.last_exit_code,
        commands: crate::interpreter::walker::clone_commands(&state.commands),
        shell_opts: state.shell_opts.clone(),
        shopt_opts: state.shopt_opts.clone(),
        limits: state.limits.clone(),
        counters: crate::interpreter::ExecutionCounters {
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
        positional_params: if positional.is_empty() {
            state.positional_params.clone()
        } else {
            positional.to_vec()
        },
        shell_name: shell_name_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| state.shell_name.clone()),
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
        pipe_stdin_bytes: None,
    };

    let result = execute_program(program, &mut sub_state);

    // Fold shared counters back into parent
    state.counters.command_count = sub_state.counters.command_count;
    state.counters.output_size = sub_state.counters.output_size;

    result
}

// ── help builtin ────────────────────────────────────────────────────

fn builtin_help(args: &[String], state: &InterpreterState) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // List all builtins with one-line descriptions
        let mut stdout = String::from("Shell builtin commands:\n\n");
        let mut names: Vec<&str> = builtin_names().to_vec();
        names.sort();
        for name in &names {
            if let Some(meta) = builtin_meta(name) {
                stdout.push_str(&format!("  {:<16} {}\n", name, meta.description));
            } else {
                stdout.push_str(&format!("  {}\n", name));
            }
        }
        return Ok(ExecResult {
            stdout,
            ..ExecResult::default()
        });
    }

    let name = &args[0];

    // Check builtins first
    if let Some(meta) = builtin_meta(name) {
        return Ok(ExecResult {
            stdout: crate::commands::format_help(meta),
            ..ExecResult::default()
        });
    }

    // Check registered commands
    if let Some(cmd) = state.commands.get(name.as_str())
        && let Some(meta) = cmd.meta()
    {
        return Ok(ExecResult {
            stdout: crate::commands::format_help(meta),
            ..ExecResult::default()
        });
    }

    Ok(ExecResult {
        stderr: format!("help: no help topics match '{}'\n", name),
        exit_code: 1,
        ..ExecResult::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::{ExecutionCounters, ExecutionLimits, ShellOpts, ShoptOpts};
    use crate::network::NetworkPolicy;
    use crate::platform::Instant;
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
            shopt_opts: ShoptOpts::default(),
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
            dir_stack: Vec::new(),
            command_hash: HashMap::new(),
            aliases: HashMap::new(),
            current_lineno: 0,
            shell_start_time: Instant::now(),
            last_argument: String::new(),
            call_stack: Vec::new(),
            machtype: "x86_64-pc-linux-gnu".to_string(),
            hosttype: "x86_64".to_string(),
            persistent_fds: HashMap::new(),
            next_auto_fd: 10,
            proc_sub_counter: 0,
            proc_sub_prealloc: HashMap::new(),
            pipe_stdin_bytes: None,
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
                value: VariableValue::Scalar("/home/user".to_string()),
                attrs: VariableAttrs::EXPORTED,
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
                value: VariableValue::Scalar("/home/user".to_string()),
                attrs: VariableAttrs::EXPORTED,
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
        assert!(state.env.get("FOO").unwrap().exported());
        assert_eq!(state.env.get("FOO").unwrap().value.as_scalar(), "bar");
    }

    #[test]
    fn unset_variable() {
        let mut state = make_state();
        set_variable(&mut state, "FOO", "bar".to_string()).unwrap();
        builtin_unset(&["FOO".to_string()], &mut state).unwrap();
        assert!(!state.env.contains_key("FOO"));
    }

    #[test]
    fn unset_readonly_fails() {
        let mut state = make_state();
        state.env.insert(
            "FOO".to_string(),
            Variable {
                value: VariableValue::Scalar("bar".to_string()),
                attrs: VariableAttrs::READONLY,
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
        assert!(state.env.get("FOO").unwrap().readonly());
        assert_eq!(state.env.get("FOO").unwrap().value.as_scalar(), "bar");
    }

    #[test]
    fn declare_readonly() {
        let mut state = make_state();
        builtin_declare(&["-r".to_string(), "X=42".to_string()], &mut state).unwrap();
        assert!(state.env.get("X").unwrap().readonly());
    }

    #[test]
    fn read_single_var() {
        let mut state = make_state();
        builtin_read(&["NAME".to_string()], &mut state, "hello world\n").unwrap();
        assert_eq!(
            state.env.get("NAME").unwrap().value.as_scalar(),
            "hello world"
        );
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
        assert_eq!(state.env.get("A").unwrap().value.as_scalar(), "one");
        assert_eq!(state.env.get("B").unwrap().value.as_scalar(), "two three");
    }

    #[test]
    fn read_reply_default() {
        let mut state = make_state();
        builtin_read(&[], &mut state, "test input\n").unwrap();
        assert_eq!(
            state.env.get("REPLY").unwrap().value.as_scalar(),
            "test input"
        );
    }

    #[test]
    fn read_eof_returns_1() {
        let mut state = make_state();
        let result = builtin_read(&["VAR".to_string()], &mut state, "").unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn read_into_array() {
        let mut state = make_state();
        builtin_read(
            &["-r".to_string(), "-a".to_string(), "arr".to_string()],
            &mut state,
            "a b c\n",
        )
        .unwrap();
        let var = state.env.get("arr").unwrap();
        match &var.value {
            VariableValue::IndexedArray(map) => {
                assert_eq!(map.get(&0).unwrap(), "a");
                assert_eq!(map.get(&1).unwrap(), "b");
                assert_eq!(map.get(&2).unwrap(), "c");
                assert_eq!(map.len(), 3);
            }
            _ => panic!("expected indexed array"),
        }
    }

    #[test]
    fn read_delimiter() {
        let mut state = make_state();
        builtin_read(
            &["-d".to_string(), ":".to_string(), "x".to_string()],
            &mut state,
            "a:b:c",
        )
        .unwrap();
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "a");
    }

    #[test]
    fn read_delimiter_empty_reads_until_eof() {
        let mut state = make_state();
        builtin_read(
            &["-d".to_string(), "".to_string(), "x".to_string()],
            &mut state,
            "hello\nworld",
        )
        .unwrap();
        assert_eq!(
            state.env.get("x").unwrap().value.as_scalar(),
            "hello\nworld"
        );
    }

    #[test]
    fn read_n_count() {
        let mut state = make_state();
        builtin_read(
            &["-n".to_string(), "3".to_string(), "x".to_string()],
            &mut state,
            "hello\n",
        )
        .unwrap();
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "hel");
    }

    #[test]
    fn read_n_stops_at_newline() {
        let mut state = make_state();
        builtin_read(
            &["-n".to_string(), "10".to_string(), "x".to_string()],
            &mut state,
            "hi\nthere\n",
        )
        .unwrap();
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "hi");
    }

    #[test]
    fn read_big_n_includes_newlines() {
        let mut state = make_state();
        builtin_read(
            &["-N".to_string(), "4".to_string(), "x".to_string()],
            &mut state,
            "ab\ncd",
        )
        .unwrap();
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "ab\nc");
    }

    #[test]
    fn read_silent_flag_accepted() {
        let mut state = make_state();
        let result = builtin_read(
            &["-s".to_string(), "VAR".to_string()],
            &mut state,
            "secret\n",
        )
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(state.env.get("VAR").unwrap().value.as_scalar(), "secret");
    }

    #[test]
    fn read_timeout_stub_with_data() {
        let mut state = make_state();
        let result = builtin_read(
            &["-t".to_string(), "1".to_string(), "VAR".to_string()],
            &mut state,
            "data\n",
        )
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(state.env.get("VAR").unwrap().value.as_scalar(), "data");
    }

    #[test]
    fn read_timeout_stub_no_data() {
        let mut state = make_state();
        let result = builtin_read(
            &["-t".to_string(), "1".to_string(), "VAR".to_string()],
            &mut state,
            "",
        )
        .unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn read_combined_ra_flags() {
        let mut state = make_state();
        builtin_read(
            &["-ra".to_string(), "arr".to_string()],
            &mut state,
            "x y z\n",
        )
        .unwrap();
        let var = state.env.get("arr").unwrap();
        match &var.value {
            VariableValue::IndexedArray(map) => {
                assert_eq!(map.len(), 3);
                assert_eq!(map.get(&0).unwrap(), "x");
                assert_eq!(map.get(&1).unwrap(), "y");
                assert_eq!(map.get(&2).unwrap(), "z");
            }
            _ => panic!("expected indexed array"),
        }
    }

    #[test]
    fn read_delimiter_not_found_returns_1() {
        let mut state = make_state();
        let result = builtin_read(
            &["-d".to_string(), ":".to_string(), "x".to_string()],
            &mut state,
            "abc",
        )
        .unwrap();
        assert_eq!(result.exit_code, 1);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "abc");
    }

    #[test]
    fn read_delimiter_empty_returns_1() {
        let mut state = make_state();
        let result = builtin_read(
            &["-d".to_string(), "".to_string(), "x".to_string()],
            &mut state,
            "hello\nworld",
        )
        .unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn read_big_n_short_read_returns_1() {
        let mut state = make_state();
        let result = builtin_read(
            &["-N".to_string(), "10".to_string(), "x".to_string()],
            &mut state,
            "ab",
        )
        .unwrap();
        assert_eq!(result.exit_code, 1);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "ab");
    }

    #[test]
    fn read_big_n_preserves_backslash() {
        let mut state = make_state();
        builtin_read(
            &["-N".to_string(), "4".to_string(), "x".to_string()],
            &mut state,
            "a\\bc",
        )
        .unwrap();
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "a\\bc");
    }

    #[test]
    fn read_n_zero_assigns_empty() {
        let mut state = make_state();
        let result = builtin_read(
            &["-n".to_string(), "0".to_string(), "x".to_string()],
            &mut state,
            "hello\n",
        )
        .unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "");
    }

    #[test]
    fn read_big_n_clears_extra_vars() {
        let mut state = make_state();
        builtin_read(
            &[
                "-N".to_string(),
                "4".to_string(),
                "a".to_string(),
                "b".to_string(),
            ],
            &mut state,
            "abcd",
        )
        .unwrap();
        assert_eq!(state.env.get("a").unwrap().value.as_scalar(), "abcd");
        assert_eq!(state.env.get("b").unwrap().value.as_scalar(), "");
    }

    #[test]
    fn resolve_relative_path() {
        assert_eq!(resolve_path("/home/user", "docs"), "/home/user/docs");
        assert_eq!(resolve_path("/home/user", ".."), "/home");
        assert_eq!(resolve_path("/home/user", "/tmp"), "/tmp");
    }

    #[test]
    fn builtin_names_is_nonempty() {
        assert!(
            !builtin_names().is_empty(),
            "builtin_names() should list at least one builtin"
        );
        // is_builtin() derives from builtin_names(), so consistency is guaranteed.
        for &name in builtin_names() {
            assert!(is_builtin(name));
        }
    }

    #[test]
    fn all_builtins_have_meta() {
        let missing: Vec<&str> = builtin_names()
            .iter()
            .filter(|&&name| builtin_meta(name).is_none())
            .copied()
            .collect();
        assert!(missing.is_empty(), "Builtins missing meta: {:?}", missing);
    }
}
