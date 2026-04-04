//! Shell builtins that modify interpreter state.

use crate::commands::CommandMeta;
use crate::error::RustBashError;
use crate::interpreter::walker::{execute_program, execute_program_with_stdin};
use crate::interpreter::{
    ControlFlow, ExecResult, InterpreterState, Variable, VariableAttrs, VariableValue,
    ensure_nested_shell_startup_vars, fold_child_process_state, next_child_pid, parse,
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
    extra_input_fds: Option<&std::collections::HashMap<i32, String>>,
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
        "read" => builtin_read(args, state, stdin, extra_input_fds).map(Some),
        "eval" => builtin_eval(args, state, stdin).map(Some),
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
        "command" => builtin_command(args, state, stdin, extra_input_fds).map(Some),
        "builtin" => builtin_builtin(args, state, stdin, extra_input_fds).map(Some),
        "getopts" => builtin_getopts(args, state).map(Some),
        "mapfile" | "readarray" => builtin_mapfile(args, state, stdin).map(Some),
        "pushd" => builtin_pushd(args, state).map(Some),
        "popd" => builtin_popd(args, state).map(Some),
        "dirs" => builtin_dirs(args, state).map(Some),
        "hash" => builtin_hash(args, state).map(Some),
        "wait" => builtin_wait(args, state).map(Some),
        "alias" => builtin_alias(args, state).map(Some),
        "unalias" => builtin_unalias(args, state).map(Some),
        "printf" => builtin_printf(args, state).map(Some),
        "sh" | "bash" => builtin_sh(args, state, stdin).map(Some),
        "help" => builtin_help(args, state).map(Some),
        "history" => builtin_history(args, state).map(Some),
        _ => Ok(None),
    }
}

/// Check if a name is a known shell builtin.
/// Derives from `builtin_names()` to keep a single source of truth.
pub(crate) fn is_builtin(name: &str) -> bool {
    builtin_names().contains(&name)
}

const SPECIAL_BUILTINS: &[&str] = &[
    ".", "source", ":", "colon", "break", "continue", "eval", "exec", "exit", "export", "readonly",
    "return", "set", "shift", "trap", "unset",
];

const INTROSPECTION_ONLY_BUILTINS: &[&str] = &["echo", "pwd", "test", "[", "true", "false"];

pub(crate) fn is_special_builtin(name: &str) -> bool {
    SPECIAL_BUILTINS.contains(&name)
}

/// Check if `name` is a valid shell variable name (alphanumerics and underscores, no leading digit).
fn is_valid_var_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

/// Extract the base variable name from an argument like `name=val`, `name+=val`,
/// or `name[idx]=val` and validate it.  Returns `Ok(())` if valid, or an error
/// message string suitable for stderr if invalid.
fn validate_var_arg(arg: &str, builtin: &str) -> Result<(), String> {
    let var_name = if let Some((n, _)) = arg.split_once("+=") {
        n
    } else if let Some((n, _)) = arg.split_once('=') {
        n
    } else {
        arg
    };
    // Strip array subscript for validation: name[idx] → name
    let base_name = var_name.split('[').next().unwrap_or(var_name);
    if is_valid_var_name(base_name) {
        Ok(())
    } else {
        Err(format!(
            "rust-bash: {builtin}: `{arg}': not a valid identifier\n"
        ))
    }
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
        Err(result) => {
            if state.loop_depth > 0 {
                state.should_exit = true;
            }
            return Ok(result);
        }
    };
    if state.loop_depth == 0 {
        return Ok(ExecResult {
            stderr: "break: only meaningful in a `for', `while', or `until' loop\n".to_string(),
            exit_code: 0,
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
    if args.len() > 1 && state.loop_depth > 0 {
        // Bash treats `continue 1 2 3` as a successful break-like escape from
        // the current loop, which the imported Oils fixture models as a bug.
        state.control_flow = Some(ControlFlow::Break(1));
        return Ok(ExecResult::default());
    }

    let n = parse_loop_level("continue", args)?;
    let n = match n {
        Ok(level) => level,
        Err(result) => {
            if state.loop_depth > 0 {
                state.should_exit = true;
            }
            return Ok(result);
        }
    };
    if state.loop_depth == 0 {
        return Ok(ExecResult {
            stderr: "continue: only meaningful in a `for', `while', or `until' loop\n".to_string(),
            exit_code: 0,
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
    // Skip -- if present
    let effective_args: &[String] = if args.first().is_some_and(|a| a == "--") {
        &args[1..]
    } else {
        args
    };

    // bash rejects cd with 2+ positional arguments
    if effective_args.len() > 1 {
        return Ok(ExecResult {
            stderr: "cd: too many arguments\n".to_string(),
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    let target = if effective_args.is_empty() {
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
    } else if effective_args[0] == "-" {
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
        effective_args[0].clone()
    };

    // CDPATH support: if target is relative and not starting with ./,
    // try CDPATH directories first.
    let mut cd_printed_path = String::new();
    let resolved = if !target.starts_with('/')
        && !target.starts_with("./")
        && !target.starts_with("../")
        && target != "."
        && target != ".."
    {
        if let Some(cdpath_var) = state.env.get("CDPATH") {
            let cdpath = cdpath_var.value.as_scalar().to_string();
            let mut found = None;
            for dir in cdpath.split(':') {
                let base = if dir.is_empty() { "." } else { dir };
                let candidate = resolve_path(
                    &state.cwd,
                    &format!("{}/{}", base.trim_end_matches('/'), &target),
                );
                let path = Path::new(&candidate);
                if state.fs.exists(path)
                    && state
                        .fs
                        .stat(path)
                        .is_ok_and(|m| m.node_type == NodeType::Directory)
                {
                    cd_printed_path = candidate.clone();
                    found = Some(candidate);
                    break;
                }
            }
            found.unwrap_or_else(|| resolve_path(&state.cwd, &target))
        } else {
            resolve_path(&state.cwd, &target)
        }
    } else {
        resolve_path(&state.cwd, &target)
    };

    // Validate intermediate path components (cd BAD/.. should fail)
    if target.contains('/') && !target.starts_with('/') {
        let components: Vec<&str> = target.split('/').collect();
        let mut check_path = state.cwd.clone();
        for (i, comp) in components.iter().enumerate() {
            if *comp == "." || comp.is_empty() {
                continue;
            }
            if *comp == ".." {
                // .. is valid if we have a parent
                continue;
            }
            check_path = resolve_path(&check_path, comp);
            // Only check intermediate components, not the final target
            if i < components.len() - 1 && !state.fs.exists(Path::new(&check_path)) {
                return Ok(ExecResult {
                    stderr: format!("cd: {target}: No such file or directory\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
        }
    }

    // Validate the final path exists and is a directory
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

    // Print directory if cd - or CDPATH match
    let stdout = if (!effective_args.is_empty() && effective_args[0] == "-")
        || !cd_printed_path.is_empty()
    {
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
    let mut exit_code = 0;
    let mut stderr = String::new();
    for arg in args {
        if arg == "-n" {
            unexport = true;
            continue;
        }
        if arg.starts_with('-') && !arg.contains('=') {
            continue; // skip other flags
        }

        if let Err(msg) = validate_var_arg(arg, "export") {
            stderr.push_str(&msg);
            exit_code = 1;
            continue;
        }

        if let Some((name, value)) = arg.split_once("+=") {
            match declare_append_value(state, name, value, VariableAttrs::EXPORTED, false, false) {
                Ok(()) => {}
                Err(RustBashError::Execution(msg)) => {
                    stderr.push_str(&format!("rust-bash: {msg}\n"));
                    exit_code = 1;
                    continue;
                }
                Err(other) => return Err(other),
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

    Ok(ExecResult {
        exit_code,
        stderr,
        ..ExecResult::default()
    })
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
            if !is_valid_var_name(name) {
                return Ok(ExecResult {
                    stderr: format!("unset: `{arg}': not a valid identifier\n"),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
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
                if index_str.contains('"') || index_str.contains('\'') {
                    let ln = state.current_lineno;
                    return Ok(ExecResult {
                        stderr: format!(
                            "rust-bash: line {ln}: unset: [{index_str}]: bad array subscript\n"
                        ),
                        exit_code: 1,
                        ..ExecResult::default()
                    });
                }
                let idx = match crate::interpreter::arithmetic::eval_arithmetic(index_str, state) {
                    Ok(idx) => idx,
                    Err(_) => {
                        let ln = state.current_lineno;
                        return Ok(ExecResult {
                            stderr: format!(
                                "rust-bash: line {ln}: unset: [{index_str}]: bad array subscript\n"
                            ),
                            exit_code: 1,
                            ..ExecResult::default()
                        });
                    }
                };
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
                            let ln = state.current_lineno;
                            return Ok(ExecResult {
                                stderr: format!(
                                    "rust-bash: line {ln}: unset: [{index_str}]: bad array subscript\n"
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
            } else if is_scalar {
                if index_str == "0" {
                    if let Some(var) = state.env.get_mut(name) {
                        var.value = VariableValue::Scalar(String::new());
                    }
                } else {
                    let ln = state.current_lineno;
                    return Ok(ExecResult {
                        stderr: format!(
                            "rust-bash: line {ln}: unset: [{index_str}]: bad array subscript\n"
                        ),
                        exit_code: 1,
                        ..ExecResult::default()
                    });
                }
            }
            continue;
        }
        if !is_valid_var_name(arg) {
            return Ok(ExecResult {
                stderr: format!("unset: `{arg}': not a valid identifier\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
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
        // In bash, bare `unset name` prefers variables. If no variable exists,
        // fall back to unsetting a function with the same name.
        if state.env.contains_key(arg.as_str()) {
            let local_scope_match =
                state
                    .local_scopes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(idx, scope)| {
                        scope.get(arg.as_str()).cloned().map(|saved| (idx, saved))
                    });
            let temp_original = current_temp_binding_original(state, arg.as_str());

            if !state.shopt_opts.localvar_unset
                && let Some((scope_idx, saved_scope_value)) = local_scope_match
            {
                let current_scope_idx = state.local_scopes.len().saturating_sub(1);
                if scope_idx < current_scope_idx {
                    match saved_scope_value {
                        Some(var) => {
                            state.env.insert(arg.clone(), var);
                        }
                        None => {
                            state.env.remove(arg.as_str());
                        }
                    }
                } else if let Some(saved) = temp_original {
                    match saved {
                        Some(var) => {
                            state.env.insert(arg.clone(), var);
                        }
                        None => {
                            state.env.remove(arg.as_str());
                        }
                    }
                } else if let Some(var) = state.env.get(arg.as_str()).cloned() {
                    state
                        .env
                        .insert(arg.clone(), declared_only_shadow_variable(&var));
                }
                continue;
            }
            match temp_original {
                Some(Some(_)) if state.shell_opts.posix && state.in_function_depth == 0 => {
                    state.env.remove(arg.as_str());
                }
                Some(Some(var)) => {
                    state.env.insert(arg.clone(), var);
                }
                Some(None) | None => {
                    state.env.remove(arg.as_str());
                }
            }
        } else {
            state.functions.remove(arg.as_str());
        }
    }
    Ok(ExecResult::default())
}

fn current_temp_binding_original(state: &InterpreterState, name: &str) -> Option<Option<Variable>> {
    state
        .temp_binding_scopes
        .iter()
        .rev()
        .find_map(|scope| scope.get(name).cloned())
}

fn saved_local_restore_value(state: &InterpreterState, name: &str) -> Option<Variable> {
    match current_temp_binding_original(state, name) {
        Some(saved) => saved,
        None => state.env.get(name).cloned(),
    }
}

fn declared_only_shadow_variable(var: &Variable) -> Variable {
    let value = match &var.value {
        VariableValue::Scalar(_) => VariableValue::Scalar(String::new()),
        VariableValue::IndexedArray(_) => {
            VariableValue::IndexedArray(std::collections::BTreeMap::new())
        }
        VariableValue::AssociativeArray(_) => {
            VariableValue::AssociativeArray(std::collections::BTreeMap::new())
        }
    };

    let mut attrs = var.attrs;
    attrs.insert(VariableAttrs::DECLARED_ONLY);

    Variable { value, attrs }
}

// ── set ─────────────────────────────────────────────────────────────

fn shell_word_needs_ansi_c_quote(value: &str) -> bool {
    value.chars().any(|ch| {
        crate::shell_bytes::marker_byte(ch).is_some() || ch == '\'' || ch == '\\' || ch.is_control()
    })
}

fn shell_word_is_plain(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | '=' | '+')
        })
}

fn shell_ansi_c_quote(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if let Some(byte) = crate::shell_bytes::marker_byte(ch) {
            out.push_str(&format!("\\{:03o}", byte));
            continue;
        }
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\x0B' => out.push_str("\\v"),
            '\x0C' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\x1B' => out.push_str("\\E"),
            control if control.is_control() => out.push_str(&format!("\\{:03o}", control as u32)),
            _ => out.push(ch),
        }
    }
    format!("$'{out}'")
}

fn shell_quote_for_reparse(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if shell_word_needs_ansi_c_quote(value) {
        return shell_ansi_c_quote(value);
    }
    if shell_word_is_plain(value) {
        return value.to_string();
    }
    format!("'{value}'")
}

fn shell_double_quote(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' | '"' | '$' | '`' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    format!("\"{out}\"")
}

fn shell_quote_for_set_array_value(value: &str) -> String {
    if shell_word_needs_ansi_c_quote(value) {
        shell_ansi_c_quote(value)
    } else {
        shell_double_quote(value)
    }
}

fn shell_quote_for_set_assoc_key(key: &str) -> String {
    if shell_word_is_plain(key) {
        key.to_string()
    } else if shell_word_needs_ansi_c_quote(key) {
        shell_ansi_c_quote(key)
    } else {
        shell_double_quote(key)
    }
}

fn format_set_line(name: &str, var: &Variable) -> String {
    match &var.value {
        VariableValue::Scalar(s) => format!("{name}={}\n", shell_quote_for_reparse(s)),
        VariableValue::IndexedArray(map) => {
            let elems: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("[{k}]={}", shell_quote_for_set_array_value(v)))
                .collect();
            format!("{name}=({})\n", elems.join(" "))
        }
        VariableValue::AssociativeArray(map) => {
            let mut entries: Vec<(&String, &String)> = map.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            let elems: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    let key = shell_quote_for_set_assoc_key(k);
                    format!("[{key}]={}", shell_quote_for_set_array_value(v))
                })
                .collect();
            if elems.is_empty() {
                format!("{name}=()\n")
            } else {
                format!("{name}=({} )\n", elems.join(" "))
            }
        }
    }
}

fn builtin_set(args: &[String], state: &mut InterpreterState) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        // List all variables in a form that can be re-sourced by `eval`/`.`.
        let mut lines: Vec<String> = state
            .env
            .iter()
            .map(|(k, v)| format_set_line(k, v))
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
        } else if arg == "-" {
            state.shell_opts.xtrace = false;
            state.shell_opts.verbose = false;
            if i + 1 < args.len() {
                state.positional_params = args[i + 1..].to_vec();
            }
            return Ok(ExecResult::default());
        } else if arg == "+" {
            if i + 1 < args.len() && args[i + 1] == "-" {
                state.positional_params = vec!["+".to_string()];
                return Ok(ExecResult::default());
            }
            i += 1;
            continue;
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
        'e' => {
            state.shell_opts.errexit = enable;
            if enable && state.in_function_depth > 0 && state.errexit_bang_suppressed > 0 {
                state.errexit_bang_suppressed = state.errexit_bang_suppressed.saturating_sub(1);
            }
        }
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
        "errexit" => {
            state.shell_opts.errexit = enable;
            if enable && state.in_function_depth > 0 && state.errexit_bang_suppressed > 0 {
                state.errexit_bang_suppressed = state.errexit_bang_suppressed.saturating_sub(1);
            }
        }
        "nounset" => state.shell_opts.nounset = enable,
        "pipefail" => state.shell_opts.pipefail = enable,
        "xtrace" => state.shell_opts.xtrace = enable,
        "verbose" => state.shell_opts.verbose = enable,
        "noexec" => state.shell_opts.noexec = enable,
        "noclobber" => state.shell_opts.noclobber = enable,
        "allexport" => state.shell_opts.allexport = enable,
        "noglob" => state.shell_opts.noglob = enable,
        "posix" => state.shell_opts.posix = enable,
        "vi" => {
            state.shell_opts.vi_mode = enable;
            if enable {
                state.shell_opts.emacs_mode = false;
            }
        }
        "emacs" => {
            state.shell_opts.emacs_mode = enable;
            if enable {
                state.shell_opts.vi_mode = false;
            }
        }
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

fn builtin_wait(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let status = if args.is_empty() {
        state.last_background_status.unwrap_or(0)
    } else {
        let pid = match args[0].parse::<u32>() {
            Ok(pid) => pid,
            Err(_) => {
                return Ok(ExecResult {
                    stderr: format!("wait: {}: not a pid or valid job spec\n", args[0]),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
        };
        if Some(pid) == state.last_background_pid {
            state.last_background_status.unwrap_or(0)
        } else {
            127
        }
    };

    state.last_exit_code = status;
    Ok(ExecResult {
        exit_code: status,
        ..ExecResult::default()
    })
}

// ── readonly ────────────────────────────────────────────────────────

fn builtin_readonly(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    // Parse flags: -p (print), -a (indexed array), -A (associative array)
    // Note: in bash, `readonly -a name` (without assignment) does NOT create
    // the variable as an array — it just marks it readonly. We parse but
    // ignore the -a/-A flags for the no-value case.
    let mut print_mode = false;
    let mut var_args: Vec<&String> = Vec::new();

    for arg in args {
        if let Some(flags) = arg.strip_prefix('-') {
            if flags.is_empty() {
                var_args.push(arg);
                continue;
            }
            for c in flags.chars() {
                match c {
                    'p' => print_mode = true,
                    'a' | 'A' => { /* accepted but not used for readonly */ }
                    _ => {}
                }
            }
        } else {
            var_args.push(arg);
        }
    }

    if print_mode {
        if var_args.is_empty() {
            // readonly -p → print all readonly variables
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
        // readonly -p varname → bash outputs nothing (bash bug: no per-var
        // readonly -p). We match this behavior.
        return Ok(ExecResult::default());
    }

    if var_args.is_empty() {
        // readonly with no args and no -p → same as readonly -p
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

    let mut exit_code = 0;
    let mut stderr = String::new();
    for arg in var_args {
        if let Err(msg) = validate_var_arg(arg, "readonly") {
            stderr.push_str(&msg);
            exit_code = 1;
            continue;
        }
        if let Some((name, value)) = arg.split_once("+=") {
            match declare_append_value(state, name, value, VariableAttrs::READONLY, false, false) {
                Ok(()) => {}
                Err(RustBashError::Execution(msg)) => {
                    stderr.push_str(&format!("rust-bash: {msg}\n"));
                    exit_code = 1;
                    continue;
                }
                Err(other) => return Err(other),
            }
        } else if let Some((name, value)) = arg.split_once('=') {
            if let Err(e) = set_variable(state, name, value.to_string()) {
                stderr.push_str(&format!("{e}\n"));
                exit_code = 1;
                continue;
            }
            if let Some(var) = state.env.get_mut(name) {
                var.attrs.insert(VariableAttrs::READONLY);
            }
        } else {
            // Mark existing variable as readonly (and set array type if requested)
            let flag_attrs = VariableAttrs::READONLY;
            // In bash, `readonly -a name` (without assignment) does NOT create
            // the variable as an array — it just marks it readonly.
            declare_without_value(state, arg, flag_attrs, false, false)?;
            // Ensure READONLY is always set even if variable already existed
            if let Some(var) = state.env.get_mut(arg.as_str()) {
                var.attrs.insert(VariableAttrs::READONLY);
            }
        }
    }

    Ok(ExecResult {
        exit_code,
        stderr,
        ..ExecResult::default()
    })
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
    let mut remove_readonly = false; // +r
    let mut remove_nameref = false; // +n
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
                    _ => {
                        return Ok(ExecResult {
                            stderr: format!("rust-bash: declare: -{c}: invalid option\n"),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                }
            }
        } else if let Some(flags) = arg.strip_prefix('+') {
            for c in flags.chars() {
                match c {
                    'x' => remove_exported = true,
                    'r' => remove_readonly = true,
                    'n' => remove_nameref = true,
                    _ => {
                        return Ok(ExecResult {
                            stderr: format!("rust-bash: declare: +{c}: invalid option\n"),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
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

    // typeset +x / +r — remove attributes
    // Note: +r is accepted but ignored per bash behavior (readonly cannot
    // be removed once set).
    if remove_exported || remove_readonly || remove_nameref {
        for arg in &var_args {
            let (name, opt_value) = if let Some((n, v)) = arg.split_once('=') {
                (n, Some(v))
            } else {
                (arg.as_str(), None)
            };
            if remove_exported && let Some(var) = state.env.get_mut(name) {
                var.attrs.remove(VariableAttrs::EXPORTED);
            }
            if remove_nameref && let Some(var) = state.env.get_mut(name) {
                var.attrs.remove(VariableAttrs::NAMEREF);
            }
            // +r: only assign value if the variable is not readonly
            if let Some(value) = opt_value {
                let is_ro = state.env.get(name).is_some_and(|v| v.readonly());
                if !is_ro {
                    set_variable(state, name, value.to_string())?;
                }
            }
        }
        return Ok(ExecResult::default());
    }

    // When `declare` is used inside a function without `-g`, the variable
    // is local to the function (same as `local`). With `-g`, it's global.
    let implicit_local = state.in_function_depth > 0 && !global_mode;

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

    // `declare -a` / `declare -A` / etc. with no var args = list matching vars
    if var_args.is_empty() {
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

    let mut exit_code = 0;
    let mut result_stderr = String::new();
    for arg in var_args {
        if let Err(msg) = validate_var_arg(arg, "declare") {
            result_stderr.push_str(&msg);
            exit_code = 1;
            continue;
        }

        // Check for += (append) before = (assign)
        if let Some((name, value)) = arg.split_once("+=") {
            if implicit_local {
                let saved = saved_local_restore_value(state, name);
                if let Some(scope) = state.local_scopes.last_mut() {
                    scope.entry(name.to_string()).or_insert(saved);
                }
            }
            match declare_append_value(
                state,
                name,
                value,
                flag_attrs,
                make_assoc_array,
                make_indexed_array,
            ) {
                Ok(()) => {}
                Err(RustBashError::Execution(msg)) => {
                    result_stderr.push_str(&format!("rust-bash: {msg}\n"));
                    exit_code = 1;
                }
                Err(other) => return Err(other),
            }
        } else if let Some((name, value)) = arg.split_once('=') {
            if implicit_local {
                let saved = saved_local_restore_value(state, name);
                if let Some(scope) = state.local_scopes.last_mut() {
                    scope.entry(name.to_string()).or_insert(saved);
                }
            }
            match declare_with_value(
                state,
                name,
                value,
                flag_attrs,
                make_assoc_array,
                make_indexed_array,
                make_nameref,
            ) {
                Ok(()) => {}
                Err(RustBashError::Execution(msg)) => {
                    result_stderr.push_str(&format!("rust-bash: {msg}\n"));
                    exit_code = 1;
                }
                Err(other) => return Err(other),
            }
        } else {
            if implicit_local {
                let saved = saved_local_restore_value(state, arg);
                if let Some(scope) = state.local_scopes.last_mut() {
                    scope.entry(arg.to_string()).or_insert(saved);
                }
            }
            match declare_without_value(
                state,
                arg,
                flag_attrs,
                make_assoc_array,
                make_indexed_array,
            ) {
                Ok(()) => {}
                Err(RustBashError::Execution(msg)) => {
                    result_stderr.push_str(&format!("rust-bash: {msg}\n"));
                    exit_code = 1;
                }
                Err(other) => return Err(other),
            }
        }
    }

    Ok(ExecResult {
        exit_code,
        stderr: result_stderr,
        ..ExecResult::default()
    })
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
        if let Some(func) = state.functions.get(name.as_str()) {
            if names_only {
                if state.shopt_opts.extdebug {
                    stdout.push_str(&format!("{name} {} {}\n", func.lineno, func.source));
                } else {
                    // `declare -F name` outputs just the name (no `declare -f` prefix).
                    stdout.push_str(&format!("{name}\n"));
                }
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
            // Filter by attribute, sort by name
            let mut entries: Vec<(&String, &Variable)> = state
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
                .collect();
            entries.sort_by_key(|(name, _)| name.as_str());
            let stdout: String = entries
                .iter()
                .map(|(k, v)| format_declare_line(k, v))
                .collect();
            return Ok(ExecResult {
                stdout,
                ..ExecResult::default()
            });
        }
        // `declare -p` with no args — list all with declare prefix, sort by name
        let mut entries: Vec<(&String, &Variable)> = state.env.iter().collect();
        entries.sort_by_key(|(name, _)| name.as_str());
        let stdout: String = entries
            .iter()
            .map(|(k, v)| format_declare_line(k, v))
            .collect();
        return Ok(ExecResult {
            stdout,
            ..ExecResult::default()
        });
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
    // Bare `declare` uses simple `name=value` format (no `declare` prefix).
    let mut entries: Vec<(&String, &Variable)> = state.env.iter().collect();
    entries.sort_by_key(|(name, _)| name.as_str());
    let stdout: String = entries
        .iter()
        .map(|(name, var)| format_simple_line(name, var))
        .collect();
    Ok(ExecResult {
        stdout,
        ..ExecResult::default()
    })
}

/// Format `name=value` (used by bare `declare` and `local`).
fn format_simple_line(name: &str, var: &Variable) -> String {
    match &var.value {
        VariableValue::Scalar(s) => format!("{name}={}\n", shell_quote_for_reparse(s)),
        VariableValue::IndexedArray(map) => {
            let elems: Vec<String> = map
                .iter()
                .map(|(k, v)| format!("[{k}]={}", shell_quote_for_reparse(v)))
                .collect();
            format!("{name}=({})\n", elems.join(" "))
        }
        VariableValue::AssociativeArray(map) => {
            let mut entries: Vec<(&String, &String)> = map.iter().collect();
            entries.sort_by(|(a, _), (b, _)| a.cmp(b));
            let elems: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    let key = if shell_word_is_plain(k) {
                        (*k).clone()
                    } else {
                        shell_quote_for_reparse(k)
                    };
                    format!("[{key}]={}", shell_quote_for_reparse(v))
                })
                .collect();
            if elems.is_empty() {
                format!("{name}=()\n")
            } else {
                format!("{name}=({} )\n", elems.join(" "))
            }
        }
    }
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
        VariableValue::Scalar(s) => {
            if var.attrs.contains(VariableAttrs::DECLARED_ONLY) {
                format!("declare {flag_str}{name}\n")
            } else {
                let quoted = shell_quote_for_set_array_value(s);
                format!("declare {flag_str}{name}={quoted}\n")
            }
        }
        VariableValue::IndexedArray(map) => {
            if var.attrs.contains(VariableAttrs::DECLARED_ONLY) {
                format!("declare {flag_str}{name}\n")
            } else {
                let elems: Vec<String> = map
                    .iter()
                    .map(|(k, v)| {
                        let quoted = shell_quote_for_set_array_value(v);
                        format!("[{k}]={quoted}")
                    })
                    .collect();
                format!("declare {flag_str}{name}=({})\n", elems.join(" "))
            }
        }
        VariableValue::AssociativeArray(map) => {
            if var.attrs.contains(VariableAttrs::DECLARED_ONLY) {
                format!("declare {flag_str}{name}\n")
            } else {
                let mut entries: Vec<(&String, &String)> = map.iter().collect();
                if var.attrs.contains(VariableAttrs::ASSOC_REVERSE_PRINT) {
                    entries.sort_by(|(a, _), (b, _)| {
                        match (a.as_str() == "0", b.as_str() == "0") {
                            (true, true) => std::cmp::Ordering::Equal,
                            (true, false) => std::cmp::Ordering::Less,
                            (false, true) => std::cmp::Ordering::Greater,
                            (false, false) => b.cmp(a),
                        }
                    });
                } else {
                    entries.sort_by(|(a, _), (b, _)| a.cmp(b));
                }
                let elems: Vec<String> = entries
                    .iter()
                    .map(|(k, v)| {
                        let quoted_key = shell_quote_for_set_assoc_key(k);
                        let quoted_val = shell_quote_for_set_array_value(v);
                        format!("[{quoted_key}]={quoted_val}")
                    })
                    .collect();
                if elems.is_empty() {
                    format!("declare {flag_str}{name}=()\n")
                } else {
                    // Bash outputs a trailing space before closing paren
                    format!("declare {flag_str}{name}=({} )\n", elems.join(" "))
                }
            }
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
    if make_assoc_array
        && state
            .env
            .get(name)
            .is_some_and(|var| matches!(var.value, VariableValue::IndexedArray(_)))
    {
        return Err(RustBashError::Execution(format!(
            "{name}: cannot convert indexed array to associative array"
        )));
    }

    // Handle array append: name+=(val1 val2 ...)
    if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        // Check if the target is an assoc array
        let is_assoc = make_assoc_array
            || state
                .env
                .get(name)
                .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)));
        let non_readonly_attrs = flag_attrs - VariableAttrs::READONLY;

        if is_assoc {
            if !state.env.contains_key(name) {
                state.env.insert(
                    name.to_string(),
                    Variable {
                        value: VariableValue::AssociativeArray(std::collections::BTreeMap::new()),
                        attrs: non_readonly_attrs,
                    },
                );
            }
            if let Some(var) = state.env.get_mut(name)
                && let VariableValue::Scalar(s) = &var.value
            {
                let mut map = std::collections::BTreeMap::new();
                if !s.is_empty() {
                    map.insert("0".to_string(), s.clone());
                }
                var.value = VariableValue::AssociativeArray(map);
            }
            parse_and_set_assoc_array_append(state, name, inner)?;
            if flag_attrs.contains(VariableAttrs::READONLY)
                && let Some(var) = state.env.get_mut(name)
            {
                var.attrs.insert(VariableAttrs::READONLY);
            }
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
                        attrs: non_readonly_attrs,
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
                var.attrs.insert(non_readonly_attrs);
            }
            if flag_attrs.contains(VariableAttrs::READONLY)
                && let Some(var) = state.env.get_mut(name)
            {
                var.attrs.insert(VariableAttrs::READONLY);
            }
        }
    } else {
        // Scalar append
        match state.env.get(name).map(|var| &var.value) {
            Some(VariableValue::IndexedArray(map)) => {
                let current = map.get(&0).cloned().unwrap_or_default();
                crate::interpreter::set_array_element(state, name, 0, format!("{current}{value}"))?;
                if let Some(var) = state.env.get_mut(name) {
                    var.attrs.insert(flag_attrs);
                }
            }
            Some(VariableValue::AssociativeArray(_)) => {
                return Err(RustBashError::Execution(format!(
                    "{name}: cannot append scalar to associative array"
                )));
            }
            _ => {
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
    if make_assoc_array
        && state
            .env
            .get(name)
            .is_some_and(|var| matches!(var.value, VariableValue::IndexedArray(_)))
    {
        return Err(RustBashError::Execution(format!(
            "{name}: cannot convert indexed array to associative array"
        )));
    }
    if make_indexed_array
        && state
            .env
            .get(name)
            .is_some_and(|var| matches!(var.value, VariableValue::AssociativeArray(_)))
    {
        return Err(RustBashError::Execution(format!(
            "{name}: cannot convert associative array to indexed array"
        )));
    }

    if make_nameref {
        // Validate the target value.
        if !value.is_empty() && !is_valid_nameref_target(value) {
            return Err(RustBashError::Execution(format!(
                "declare: `{value}': not a valid identifier"
            )));
        }
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
        let non_readonly_attrs = flag_attrs - VariableAttrs::READONLY;
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::AssociativeArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            });
        var.attrs.insert(non_readonly_attrs);
        if !matches!(var.value, VariableValue::AssociativeArray(_)) {
            var.value = VariableValue::AssociativeArray(std::collections::BTreeMap::new());
        }
        // Parse assoc array literal: ([key1]=val1 [key2]=val2 ...)
        if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            parse_and_set_assoc_array(state, name, inner)?;
        }
        if flag_attrs.contains(VariableAttrs::READONLY)
            && let Some(var) = state.env.get_mut(name)
        {
            var.attrs.insert(VariableAttrs::READONLY);
        }
    } else if make_indexed_array {
        let non_readonly_attrs = flag_attrs - VariableAttrs::READONLY;
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            });
        var.attrs.insert(non_readonly_attrs);
        if !matches!(var.value, VariableValue::IndexedArray(_)) {
            var.value = VariableValue::IndexedArray(std::collections::BTreeMap::new());
        }
        // Parse array literal (x y z) or set element [0].
        if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            parse_and_set_indexed_array(state, name, inner)?;
        } else if !value.is_empty() {
            crate::interpreter::set_array_element(state, name, 0, value.to_string())?;
        }
        if flag_attrs.contains(VariableAttrs::READONLY)
            && let Some(var) = state.env.get_mut(name)
        {
            var.attrs.insert(VariableAttrs::READONLY);
        }
    } else if let Some(inner) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        // `declare name=(x y z)` without -a flag — auto-create indexed array.
        let non_readonly_attrs = flag_attrs - VariableAttrs::READONLY;
        let var = state
            .env
            .entry(name.to_string())
            .or_insert_with(|| Variable {
                value: VariableValue::IndexedArray(std::collections::BTreeMap::new()),
                attrs: VariableAttrs::empty(),
            });
        var.attrs.insert(non_readonly_attrs);
        if !matches!(var.value, VariableValue::IndexedArray(_)) {
            var.value = VariableValue::IndexedArray(std::collections::BTreeMap::new());
        }
        parse_and_set_indexed_array(state, name, inner)?;
        if flag_attrs.contains(VariableAttrs::READONLY)
            && let Some(var) = state.env.get_mut(name)
        {
            var.attrs.insert(VariableAttrs::READONLY);
        }
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
    // When adding the nameref attribute, validate the target value.
    if flag_attrs.contains(VariableAttrs::NAMEREF)
        && let Some(var) = state.env.get(name)
    {
        let target = var.value.as_scalar().to_string();
        if !target.is_empty() && !is_valid_nameref_target(&target) {
            return Err(RustBashError::Execution(format!(
                "declare: `{target}': not a valid identifier"
            )));
        }
        // Nameref on array is a hard error.
        if matches!(
            var.value,
            VariableValue::IndexedArray(_) | VariableValue::AssociativeArray(_)
        ) {
            return Err(RustBashError::Execution(
                "declare: nameref variable cannot be an array".to_string(),
            ));
        }
    }
    if let Some(var) = state.env.get_mut(name) {
        // Prevent converting between indexed and associative arrays.
        if make_assoc_array && matches!(var.value, VariableValue::IndexedArray(_)) {
            return Err(RustBashError::Execution(format!(
                "declare: {name}: cannot convert indexed array to associative array"
            )));
        }
        if make_indexed_array && matches!(var.value, VariableValue::AssociativeArray(_)) {
            return Err(RustBashError::Execution(format!(
                "declare: {name}: cannot convert associative array to indexed array"
            )));
        }
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
                attrs: flag_attrs | VariableAttrs::DECLARED_ONLY,
            },
        );
    }
    Ok(())
}

/// Check if a string is a valid nameref target (valid variable identifier,
/// or an array subscript like `a[2]`).
fn is_valid_nameref_target(s: &str) -> bool {
    // Allow array subscript form: name[...]
    let name_part = if let Some(bracket_pos) = s.find('[') {
        if s.ends_with(']') {
            &s[..bracket_pos]
        } else {
            return false;
        }
    } else {
        s
    };
    if name_part.is_empty() {
        return false;
    }
    let first = name_part.as_bytes()[0];
    if !(first == b'_' || first.is_ascii_alphabetic()) {
        return false;
    }
    name_part
        .bytes()
        .all(|b| b == b'_' || b.is_ascii_alphanumeric())
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
        if assoc_body_uses_unquoted_keys(&words) {
            var.attrs.insert(VariableAttrs::ASSOC_REVERSE_PRINT);
        } else {
            var.attrs.remove(VariableAttrs::ASSOC_REVERSE_PRINT);
        }
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
    if assoc_body_uses_unquoted_keys(&words)
        && let Some(var) = state.env.get_mut(name)
    {
        var.attrs.insert(VariableAttrs::ASSOC_REVERSE_PRINT);
    }
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

fn assoc_body_uses_unquoted_keys(words: &[String]) -> bool {
    words.iter().any(|word| {
        let Some(rest) = word.strip_prefix('[') else {
            return false;
        };
        let key = if let Some(eq_pos) = rest.find("]=") {
            &rest[..eq_pos]
        } else if let Some(key_str) = rest.strip_suffix(']') {
            key_str
        } else {
            return false;
        };
        !is_simple_quoted_assoc_key(key)
    })
}

fn is_simple_quoted_assoc_key(key: &str) -> bool {
    (key.starts_with('\'') && key.ends_with('\'') && key.len() >= 2)
        || (key.starts_with('"') && key.ends_with('"') && key.len() >= 2)
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
    extra_input_fds: Option<&std::collections::HashMap<i32, String>>,
) -> Result<ExecResult, RustBashError> {
    let mut raw_mode = false;
    let mut array_name: Option<String> = None;
    let mut delimiter: Option<char> = None; // None = newline (default)
    let mut n_count: Option<usize> = None; // -n count
    let mut big_n_count: Option<usize> = None; // -N count
    let mut timeout: Option<f64> = None; // -t timeout
    let mut read_fd: Option<i32> = None; // -u fd
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
                            delimiter = Some('\0');
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
                        match count_str.parse::<usize>() {
                            Ok(count) => n_count = Some(count),
                            Err(_) => {
                                return Ok(ExecResult {
                                    stderr: format!("read: {count_str}: invalid count\n"),
                                    exit_code: 1,
                                    ..ExecResult::default()
                                });
                            }
                        }
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
                        match count_str.parse::<usize>() {
                            Ok(count) => big_n_count = Some(count),
                            Err(_) => {
                                return Ok(ExecResult {
                                    stderr: format!("read: {count_str}: invalid count\n"),
                                    exit_code: 1,
                                    ..ExecResult::default()
                                });
                            }
                        }
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
                        // -t timeout
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        let timeout_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() {
                                args[i].as_str()
                            } else {
                                "0"
                            }
                        } else {
                            rest.as_str()
                        };
                        match timeout_str.parse::<f64>() {
                            Ok(value) => timeout = Some(value),
                            Err(_) => {
                                return Ok(ExecResult {
                                    stderr: format!("read: {timeout_str}: invalid timeout\n"),
                                    exit_code: 1,
                                    ..ExecResult::default()
                                });
                            }
                        }
                        j = flag_chars.len();
                        continue;
                    }
                    'u' => {
                        let rest: String = flag_chars[j + 1..].iter().collect();
                        let fd_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() { args[i].as_str() } else { "" }
                        } else {
                            rest.as_str()
                        };
                        match fd_str.parse::<i32>() {
                            Ok(fd) if fd >= 0 => read_fd = Some(fd),
                            _ => {
                                return Ok(ExecResult {
                                    stderr: "read: -u: invalid file descriptor specification\n"
                                        .to_string(),
                                    exit_code: 1,
                                    ..ExecResult::default()
                                });
                            }
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

    if let Some(timeout) = timeout {
        if timeout < 0.0 {
            return Ok(ExecResult {
                stderr: "read: timeout must be non-negative\n".to_string(),
                exit_code: 1,
                ..ExecResult::default()
            });
        }
        if timeout == 0.0 {
            return Ok(ExecResult::default());
        }
    }

    let input = resolve_read_input(read_fd, state, stdin, extra_input_fds)?;
    if input.data.is_empty() {
        return Ok(ExecResult {
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    let scan = scan_read_units(
        &input.data,
        raw_mode,
        delimiter.unwrap_or('\n'),
        n_count,
        big_n_count,
    );
    commit_read_input_consumption(state, &input.origin, scan.consumed_bytes);

    let line = read_units_to_string(&scan.units);

    // Check input line length before processing
    if line.len() > state.limits.max_string_length {
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_string_length",
            limit_value: state.limits.max_string_length,
            actual_value: line.len(),
        });
    }

    // Get IFS for splitting
    let ifs = state
        .env
        .get("IFS")
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_else(|| " \t\n".to_string());

    if let Some(ref arr_name) = array_name {
        // -a mode: split into indexed array
        let fields = if scan.units.is_empty() {
            // Empty input always produces an empty array
            Vec::new()
        } else if ifs.is_empty() {
            vec![line.clone()]
        } else {
            split_read_array_fields(&scan.units, &ifs)
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
            set_array_element(state, arr_name, idx, field.clone())?;
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
        assign_read_fields_to_vars(state, &scan.units, &ifs, &var_names)?;
    }

    Ok(ExecResult {
        exit_code: i32::from(scan.hit_eof),
        ..ExecResult::default()
    })
}

#[derive(Clone, Copy)]
struct ReadUnit {
    ch: char,
    escaped: bool,
}

enum ReadInputOrigin {
    Stdin { start_offset: usize },
    PersistentFd { fd: i32, start_offset: usize },
    TemporaryFd,
}

struct ReadInput {
    data: String,
    origin: ReadInputOrigin,
}

struct ReadScanResult {
    units: Vec<ReadUnit>,
    consumed_bytes: usize,
    hit_eof: bool,
}

fn resolve_read_input(
    read_fd: Option<i32>,
    state: &InterpreterState,
    stdin: &str,
    extra_input_fds: Option<&std::collections::HashMap<i32, String>>,
) -> Result<ReadInput, RustBashError> {
    if let Some(fd) = read_fd {
        if let Some(contents) = extra_input_fds.and_then(|fds| fds.get(&fd)) {
            return Ok(ReadInput {
                data: contents.clone(),
                origin: ReadInputOrigin::TemporaryFd,
            });
        }
        if fd == 0 {
            return default_read_input(state, stdin);
        }
        if state.persistent_fds.contains_key(&fd) {
            return persistent_fd_read_input(fd, state, stdin);
        }
        return Ok(ReadInput {
            data: String::new(),
            origin: ReadInputOrigin::TemporaryFd,
        });
    }

    default_read_input(state, stdin)
}

fn default_read_input(state: &InterpreterState, stdin: &str) -> Result<ReadInput, RustBashError> {
    if let Some(fd) = state.current_stdin_persistent_fd {
        return persistent_fd_read_input(fd, state, stdin);
    }

    let start_offset = state.stdin_offset;
    Ok(ReadInput {
        data: slice_string_from_offset(stdin, start_offset),
        origin: ReadInputOrigin::Stdin { start_offset },
    })
}

fn persistent_fd_read_input(
    fd: i32,
    state: &InterpreterState,
    stdin: &str,
) -> Result<ReadInput, RustBashError> {
    let start_offset = state.persistent_fd_offsets.get(&fd).copied().unwrap_or(0);
    let data = match state.persistent_fds.get(&fd) {
        Some(crate::interpreter::PersistentFd::InputFile(path))
        | Some(crate::interpreter::PersistentFd::ReadWriteFile(path)) => {
            let contents = state
                .fs
                .read_file(Path::new(path))
                .map_err(|e| RustBashError::Execution(e.to_string()))?;
            let contents = String::from_utf8_lossy(&contents).to_string();
            slice_string_from_offset(&contents, start_offset)
        }
        Some(crate::interpreter::PersistentFd::DupStdFd(0)) => {
            slice_string_from_offset(stdin, start_offset)
        }
        Some(crate::interpreter::PersistentFd::DevNull)
        | Some(crate::interpreter::PersistentFd::Closed)
        | Some(crate::interpreter::PersistentFd::OutputFile(_))
        | Some(crate::interpreter::PersistentFd::DupStdFd(_))
        | None => String::new(),
    };

    Ok(ReadInput {
        data,
        origin: ReadInputOrigin::PersistentFd { fd, start_offset },
    })
}

fn slice_string_from_offset(s: &str, start_offset: usize) -> String {
    if start_offset >= s.len() {
        String::new()
    } else {
        s[start_offset..].to_string()
    }
}

fn commit_read_input_consumption(
    state: &mut InterpreterState,
    origin: &ReadInputOrigin,
    consumed_bytes: usize,
) {
    match origin {
        ReadInputOrigin::Stdin { start_offset } => {
            state.stdin_offset = start_offset + consumed_bytes;
        }
        ReadInputOrigin::PersistentFd { fd, start_offset } => {
            state
                .persistent_fd_offsets
                .insert(*fd, start_offset + consumed_bytes);
        }
        ReadInputOrigin::TemporaryFd => {}
    }
}

fn scan_read_units(
    input: &str,
    raw_mode: bool,
    delimiter: char,
    n_count: Option<usize>,
    big_n_count: Option<usize>,
) -> ReadScanResult {
    if let Some(count) = big_n_count {
        let mut units = Vec::new();
        let mut consumed_bytes = 0;
        for (idx, ch) in input.char_indices() {
            if units.len() >= count {
                break;
            }
            units.push(ReadUnit { ch, escaped: false });
            consumed_bytes = idx + ch.len_utf8();
        }
        return ReadScanResult {
            hit_eof: units.len() < count,
            units,
            consumed_bytes,
        };
    }

    let chars: Vec<(usize, char)> = input.char_indices().collect();
    let mut units = Vec::new();
    let mut consumed_bytes = 0;
    let mut produced_chars = 0usize;
    let mut found_delimiter = false;
    let mut i = 0usize;

    while i < chars.len() {
        if let Some(limit) = n_count
            && produced_chars >= limit
        {
            break;
        }

        let (byte_idx, ch) = chars[i];
        let ch_end = byte_idx + ch.len_utf8();

        if !raw_mode && ch == '\\' {
            if let Some((next_idx, next)) = chars.get(i + 1).copied() {
                let next_end = next_idx + next.len_utf8();
                if next == '\n' {
                    consumed_bytes = next_end;
                    i += 2;
                    continue;
                }

                units.push(ReadUnit {
                    ch: next,
                    escaped: true,
                });
                consumed_bytes = next_end;
                produced_chars += 1;
                i += 2;
                continue;
            }

            consumed_bytes = ch_end;
            break;
        }

        if ch == delimiter {
            consumed_bytes = ch_end;
            found_delimiter = true;
            break;
        }

        units.push(ReadUnit { ch, escaped: false });
        consumed_bytes = ch_end;
        produced_chars += 1;
        i += 1;
    }

    ReadScanResult {
        hit_eof: !found_delimiter && consumed_bytes >= input.len(),
        units,
        consumed_bytes,
    }
}

fn read_units_to_string(units: &[ReadUnit]) -> String {
    units.iter().map(|unit| unit.ch).collect()
}

fn is_read_ifs_ws(unit: ReadUnit, ifs: &str) -> bool {
    !unit.escaped && matches!(unit.ch, ' ' | '\t' | '\n') && ifs.contains(unit.ch)
}

fn is_read_ifs_non_ws(unit: ReadUnit, ifs: &str) -> bool {
    !unit.escaped && ifs.contains(unit.ch) && !matches!(unit.ch, ' ' | '\t' | '\n')
}

fn is_read_ifs_delim(unit: ReadUnit, ifs: &str) -> bool {
    is_read_ifs_ws(unit, ifs) || is_read_ifs_non_ws(unit, ifs)
}

fn trim_read_ifs_ws_range(units: &[ReadUnit], ifs: &str) -> (usize, usize) {
    let mut start = 0;
    while start < units.len() && is_read_ifs_ws(units[start], ifs) {
        start += 1;
    }

    let mut end = units.len();
    while end > start && is_read_ifs_ws(units[end - 1], ifs) {
        end -= 1;
    }

    (start, end)
}

fn split_read_array_fields(units: &[ReadUnit], ifs: &str) -> Vec<String> {
    let chars: Vec<(char, bool)> = units.iter().map(|unit| (unit.ch, unit.escaped)).collect();
    crate::interpreter::expansion::split_ifs_quoted_chars(&chars, ifs)
}

/// Assign IFS-split fields to variables, preserving original text for the last variable.
/// In bash, the last variable receives the remainder of the line (not a split-and-rejoin).
fn assign_read_fields_to_vars(
    state: &mut InterpreterState,
    units: &[ReadUnit],
    ifs: &str,
    var_names: &[&str],
) -> Result<(), RustBashError> {
    if ifs.is_empty() || var_names.len() <= 1 {
        // Single variable: assign whole line
        // For REPLY (no named vars), don't trim leading/trailing whitespace
        // For a single named var, only trim IFS whitespace from edges
        let reply_uses_mksh_splitting = state.env.contains_key("BRUSH_LEGACY_KSH_REPLY");
        let value = if var_names.first().copied() == Some("REPLY") && var_names.len() == 1 {
            if reply_uses_mksh_splitting {
                let (start, end) = trim_read_ifs_ws_range(units, ifs);
                read_units_to_string(&units[start..end])
            } else {
                // REPLY: strip trailing newline but preserve other whitespace
                read_units_to_string(units)
            }
        } else if ifs.is_empty() {
            read_units_to_string(units)
        } else {
            let (start, end) = trim_read_ifs_ws_range(units, ifs);
            read_units_to_string(&units[start..end])
        };
        let var_name = var_names.first().copied().unwrap_or("REPLY");
        return set_variable(state, var_name, value);
    }

    // Multiple variables: extract fields one at a time, preserving original text for the last
    let has_ws = ifs.contains(' ') || ifs.contains('\t') || ifs.contains('\n');

    let mut pos = 0;
    // Skip leading IFS whitespace
    while has_ws && pos < units.len() && is_read_ifs_ws(units[pos], ifs) {
        pos += 1;
    }

    for (i, var_name) in var_names.iter().enumerate() {
        if i == var_names.len() - 1 {
            // Last variable: take the rest of the line, trim trailing IFS whitespace
            let mut end = units.len();
            while has_ws && end > pos && is_read_ifs_ws(units[end - 1], ifs) {
                end -= 1;
            }

            if end > pos {
                let trailing_non_ws = units[pos..end]
                    .iter()
                    .rev()
                    .take_while(|unit| is_read_ifs_non_ws(**unit, ifs))
                    .count();
                let has_unescaped_ifs_ws_before_trailing = units
                    [pos..end.saturating_sub(trailing_non_ws)]
                    .iter()
                    .any(|unit| is_read_ifs_ws(*unit, ifs));
                if trailing_non_ws == 1 && !has_unescaped_ifs_ws_before_trailing {
                    end -= 1;
                }
            }

            let remainder = &units[pos..end];
            let value = if remainder.len() == 1 && is_read_ifs_non_ws(remainder[0], ifs) {
                String::new()
            } else if remainder.len() >= 3
                && remainder
                    .iter()
                    .all(|unit| matches!(unit.ch, ' ' | '\t' | '\n'))
                && remainder.first().is_some_and(|unit| unit.escaped)
                && remainder.last().is_some_and(|unit| unit.escaped)
                && remainder.iter().any(|unit| !unit.escaped)
            {
                // Preserve bash's historical SOH quirk for this escaped-whitespace shape.
                "\u{1}".to_string()
            } else {
                read_units_to_string(remainder)
            };
            set_variable(state, var_name, value)?;
        } else {
            // Extract one field
            let field_start = pos;
            while pos < units.len() {
                if is_read_ifs_delim(units[pos], ifs) {
                    break;
                }
                pos += 1;
            }
            let field = read_units_to_string(&units[field_start..pos]);
            set_variable(state, var_name, field)?;

            // Skip separators after the field
            while has_ws && pos < units.len() && is_read_ifs_ws(units[pos], ifs) {
                pos += 1;
            }
            // Skip exactly one non-whitespace IFS delimiter if present
            if pos < units.len() && is_read_ifs_non_ws(units[pos], ifs) {
                pos += 1;
                while has_ws && pos < units.len() && is_read_ifs_ws(units[pos], ifs) {
                    pos += 1;
                }
            }
        }
    }
    Ok(())
}

// ── eval ─────────────────────────────────────────────────────────────

fn builtin_eval(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        return Ok(ExecResult::default());
    }

    // eval accepts and ignores `--`
    let args = if args.first().map(|a| a.as_str()) == Some("--") {
        &args[1..]
    } else {
        args
    };

    if args.is_empty() {
        return Ok(ExecResult::default());
    }

    if let Some(first) = args.first()
        && first.starts_with('-')
        && first != "-"
        && first != "--"
    {
        return Ok(ExecResult {
            stderr: format!("eval: {first}: invalid option\neval: usage: eval [arg ...]\n"),
            exit_code: 2,
            ..ExecResult::default()
        });
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
            let msg = format!("{e}");
            return Ok(ExecResult {
                stderr: if msg.is_empty() {
                    String::new()
                } else {
                    format!("eval: {msg}\n")
                },
                exit_code: 1,
                ..ExecResult::default()
            });
        }
    };
    let saved_source_text = std::mem::replace(&mut state.current_source_text, input);
    let result = execute_program_with_stdin(&program, state, stdin);
    state.current_source_text = saved_source_text;
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
    "assoc_expand_once",
    "autocd",
    "cdable_vars",
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
    "extdebug",
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
    "localvar_inherit",
    "localvar_unset",
    "login_shell",
    "mailwarn",
    "no_empty_cmd_completion",
    "nocaseglob",
    "nocasematch",
    "nullglob",
    "patsub_replacement",
    "progcomp",
    "progcomp_alias",
    "promptvars",
    "shift_verbose",
    "sourcepath",
    "varredir_close",
    "xpg_echo",
];

fn get_shopt(state: &InterpreterState, name: &str) -> Option<bool> {
    let o = &state.shopt_opts;
    match name {
        "assoc_expand_once" => Some(o.assoc_expand_once),
        "autocd" => Some(o.autocd),
        "cdable_vars" => Some(o.cdable_vars),
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
        "extdebug" => Some(o.extdebug),
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
        "localvar_inherit" => Some(o.localvar_inherit),
        "localvar_unset" => Some(o.localvar_unset),
        "login_shell" => Some(o.login_shell),
        "mailwarn" => Some(o.mailwarn),
        "no_empty_cmd_completion" => Some(o.no_empty_cmd_completion),
        "nocaseglob" => Some(o.nocaseglob),
        "nocasematch" => Some(o.nocasematch),
        "nullglob" => Some(o.nullglob),
        "patsub_replacement" => Some(o.patsub_replacement),
        "progcomp" => Some(o.progcomp),
        "progcomp_alias" => Some(o.progcomp_alias),
        "promptvars" => Some(o.promptvars),
        "shift_verbose" => Some(o.shift_verbose),
        "sourcepath" => Some(o.sourcepath),
        "strict_arg_parse" => Some(o.strict_arg_parse),
        "strict_argv" => Some(o.strict_argv),
        "strict_array" => Some(o.strict_array),
        "strict_arith" => Some(o.strict_arith),
        "varredir_close" => Some(o.varredir_close),
        "xpg_echo" => Some(o.xpg_echo),
        "strict:all" => {
            Some(o.strict_arg_parse && o.strict_argv && o.strict_array && o.strict_arith)
        }
        "ysh:all" => Some(false),
        _ => get_set_option(name, state),
    }
}

fn set_shopt(state: &mut InterpreterState, name: &str, value: bool) -> bool {
    let o = &mut state.shopt_opts;
    match name {
        "assoc_expand_once" => o.assoc_expand_once = value,
        "autocd" => o.autocd = value,
        "cdable_vars" => o.cdable_vars = value,
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
        "extdebug" => o.extdebug = value,
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
        "localvar_inherit" => o.localvar_inherit = value,
        "localvar_unset" => o.localvar_unset = value,
        "login_shell" => o.login_shell = value,
        "mailwarn" => o.mailwarn = value,
        "no_empty_cmd_completion" => o.no_empty_cmd_completion = value,
        "nocaseglob" => o.nocaseglob = value,
        "nocasematch" => o.nocasematch = value,
        "nullglob" => o.nullglob = value,
        "patsub_replacement" => o.patsub_replacement = value,
        "progcomp" => o.progcomp = value,
        "progcomp_alias" => o.progcomp_alias = value,
        "promptvars" => o.promptvars = value,
        "shift_verbose" => o.shift_verbose = value,
        "sourcepath" => o.sourcepath = value,
        "strict_arg_parse" => o.strict_arg_parse = value,
        "strict_argv" => o.strict_argv = value,
        "strict_array" => o.strict_array = value,
        "strict_arith" => o.strict_arith = value,
        "varredir_close" => o.varredir_close = value,
        "xpg_echo" => o.xpg_echo = value,
        "strict:all" => {
            o.strict_arg_parse = value;
            o.strict_argv = value;
            o.strict_array = value;
            o.strict_arith = value;
        }
        "ysh:all" => {}
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
    let mut set_short_flag = false;
    let mut unset_flag = false; // -u
    let mut unset_short_flag = false;
    let mut query_flag = false; // -q
    let mut print_flag = false; // -p
    let mut o_flag = false; // -o (use set -o options instead of shopt options)
    let mut opt_names: Vec<&str> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--set" {
            set_flag = true;
        } else if arg == "--unset" {
            unset_flag = true;
        } else if arg == "--query" {
            query_flag = true;
        } else if arg == "--print" {
            print_flag = true;
        } else if arg == "--" {
            opt_names.extend(args[i + 1..].iter().map(|s| s.as_str()));
            break;
        } else if arg.starts_with('-') && arg.len() > 1 && opt_names.is_empty() {
            for c in arg[1..].chars() {
                match c {
                    's' => {
                        set_flag = true;
                        set_short_flag = true;
                    }
                    'u' => {
                        unset_flag = true;
                        unset_short_flag = true;
                    }
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
        return Ok(ExecResult {
            exit_code: i32::from(set_short_flag && opt_names.contains(&"strict_arith")),
            ..ExecResult::default()
        });
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
            exit_code: exit_code
                | i32::from(unset_short_flag && opt_names.contains(&"strict_arith")),
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
                    // bash returns 1 (not 2) for invalid option with -q
                    return Ok(ExecResult {
                        stderr: format!("shopt: {name}: invalid shell option name\n"),
                        exit_code: 1,
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
        let mut stderr = String::new();
        let mut any_invalid = false;
        let mut any_unset = false;
        for name in &names {
            match get_shopt(state, name) {
                Some(val) => {
                    if !val {
                        any_unset = true;
                    }
                    if print_flag {
                        let flag = if val { "-s" } else { "-u" };
                        out.push_str(&format!("shopt {flag} {name}\n"));
                    } else {
                        let status = if val { "on" } else { "off" };
                        out.push_str(&format!("{name:<24}{status}\n"));
                    }
                }
                None => {
                    stderr.push_str(&format!("shopt: {name}: invalid shell option name\n"));
                    any_invalid = true;
                }
            }
        }
        // Exit code reflects option state for named options (both -p and no-flag modes)
        let exit_code = if any_invalid || (!no_args && any_unset) {
            1
        } else {
            0
        };
        return Ok(ExecResult {
            stdout: out,
            stderr,
            exit_code,
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
    let mut stderr = String::new();
    let mut any_invalid = false;
    let mut any_unset = false;
    for name in &names {
        match get_set_option(name, state) {
            Some(val) => {
                if !val {
                    any_unset = true;
                }
                let flag = if val { "-o" } else { "+o" };
                out.push_str(&format!("set {flag} {name}\n"));
            }
            None => {
                stderr.push_str(&format!("shopt: {name}: invalid shell option name\n"));
                any_invalid = true;
            }
        }
    }
    let exit_code = if any_invalid || (!no_args && any_unset) {
        1
    } else {
        0
    };
    Ok(ExecResult {
        stdout: out,
        stderr,
        exit_code,
        ..ExecResult::default()
    })
}

// ── source / . ──────────────────────────────────────────────────────

fn builtin_source(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    // source accepts and ignores `--`
    let args = if args.first().map(|a| a.as_str()) == Some("--") {
        &args[1..]
    } else {
        args
    };

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

    let path_value = current_path(state);
    let resolved = if path_arg.contains('/') {
        resolve_path(&state.cwd, path_arg)
    } else if let Some(path) = search_path_for_any_file_with_value(path_arg, &path_value, state) {
        resolve_path(&state.cwd, &path)
    } else {
        resolve_path(&state.cwd, path_arg)
    };
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
            let msg = format!("{e}");
            return Ok(ExecResult {
                stderr: if msg.is_empty() {
                    String::new()
                } else {
                    format!("{path_arg}: {msg}\n")
                },
                exit_code: 1,
                ..ExecResult::default()
            });
        }
    };
    let saved_source = std::mem::replace(&mut state.current_source, path_arg.to_string());
    let saved_source_text = std::mem::replace(&mut state.current_source_text, content.clone());
    let override_params = args.len() > 1;
    let saved_params = override_params.then(|| state.positional_params.clone());
    if override_params {
        state.positional_params = args[1..].to_vec();
    }
    state.source_depth += 1;
    // Push a call stack frame so FUNCNAME/BASH_SOURCE/BASH_LINENO work.
    // Use the original path argument (not the resolved absolute path) for
    // BASH_SOURCE, matching bash behavior.
    state.call_stack.push(crate::interpreter::CallFrame {
        func_name: "source".to_string(),
        source: path_arg.to_string(),
        lineno: state.current_lineno,
    });
    let result = execute_program(&program, state);
    state.call_stack.pop();
    state.source_depth = state.source_depth.saturating_sub(1);
    state.current_source = saved_source;
    state.current_source_text = saved_source_text;
    if let Some(saved_params) = saved_params {
        state.positional_params = saved_params;
    }
    state.counters.call_depth -= 1;
    match result {
        Ok(mut result) => {
            match state.control_flow.take() {
                Some(ControlFlow::Return(code)) => {
                    result.exit_code = code;
                }
                Some(other) => {
                    state.control_flow = Some(other);
                }
                None => {}
            }
            Ok(result)
        }
        Err(err) => Err(err),
    }
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

    // bare `local` with no args — list local variables in simple name=value format
    if var_args.is_empty() {
        let mut stdout = String::new();
        if let Some(scope) = state.local_scopes.last() {
            let mut names: Vec<&String> = scope.keys().collect();
            names.sort();
            for name in names {
                if let Some(var) = state.env.get(name.as_str()) {
                    stdout.push_str(&format_simple_line(name, var));
                }
            }
        }
        return Ok(ExecResult {
            stdout,
            ..ExecResult::default()
        });
    }

    let mut exit_code = 0;
    let mut result_stderr = String::new();
    for arg in &var_args {
        if let Err(msg) = validate_var_arg(arg, "local") {
            result_stderr.push_str(&msg);
            exit_code = 1;
            continue;
        }

        if let Some((raw_name, value)) = arg.split_once("+=") {
            // local name+=value — append
            let name = raw_name;
            let saved = saved_local_restore_value(state, name);
            if let Some(scope) = state.local_scopes.last_mut() {
                scope.entry(name.to_string()).or_insert(saved);
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
            let saved = saved_local_restore_value(state, name);
            if let Some(scope) = state.local_scopes.last_mut() {
                scope.entry(name.to_string()).or_insert(saved);
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
                if let Err(err) = set_variable(state, name, value.to_string()) {
                    result_stderr.push_str(&format!("{err}\n"));
                    exit_code = 1;
                    continue;
                }
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
            let saved = saved_local_restore_value(state, arg);
            let already_local = state
                .local_scopes
                .last()
                .is_some_and(|scope| scope.contains_key(arg.as_str()));
            if let Some(scope) = state.local_scopes.last_mut() {
                scope.entry(arg.to_string()).or_insert(saved);
            }
            // Inside a function: always set to empty. Outside: only if undefined.
            if !already_local
                && (state.in_function_depth > 0 || !state.env.contains_key(arg.as_str()))
            {
                let value = if make_indexed_array {
                    VariableValue::IndexedArray(std::collections::BTreeMap::new())
                } else if make_assoc_array {
                    VariableValue::AssociativeArray(std::collections::BTreeMap::new())
                } else {
                    VariableValue::Scalar(String::new())
                };
                let mut attrs = VariableAttrs::DECLARED_ONLY;
                if make_readonly {
                    attrs.insert(VariableAttrs::READONLY);
                }
                if make_exported {
                    attrs.insert(VariableAttrs::EXPORTED);
                }
                if make_integer {
                    attrs.insert(VariableAttrs::INTEGER);
                }
                if make_nameref {
                    attrs.insert(VariableAttrs::NAMEREF);
                }
                state.env.insert(arg.to_string(), Variable { value, attrs });
            }
        }
    }
    Ok(ExecResult {
        exit_code,
        stderr: result_stderr,
        ..ExecResult::default()
    })
}

// ── return ───────────────────────────────────────────────────────────

fn builtin_return(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if state.in_function_depth == 0 && state.source_depth == 0 {
        return Ok(ExecResult {
            stderr: "return: can only `return' from a function or sourced script\n".to_string(),
            exit_code: 2,
            ..ExecResult::default()
        });
    }

    let code = if let Some(arg) = args.first() {
        match arg.parse::<i64>() {
            Ok(n) => n.rem_euclid(256) as i32,
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
fn current_path(state: &InterpreterState) -> String {
    state
        .env
        .get("PATH")
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_else(|| "/usr/bin:/bin".to_string())
}

pub(crate) fn search_path_with_value(
    cmd: &str,
    path_value: &str,
    state: &InterpreterState,
) -> Option<String> {
    if cmd.is_empty() {
        return None;
    }

    if cmd.contains('/') {
        let resolved = resolve_path(&state.cwd, cmd);
        return is_executable_path(&resolved, state).then(|| cmd.to_string());
    }

    for dir in path_value.split(':') {
        let candidate = if dir.is_empty() {
            format!("./{cmd}")
        } else {
            format!("{dir}/{cmd}")
        };
        let resolved = resolve_path(&state.cwd, &candidate);
        if is_executable_path(&resolved, state) {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn search_path(cmd: &str, state: &InterpreterState) -> Option<String> {
    let path_value = current_path(state);
    search_path_with_value(cmd, &path_value, state)
}

pub(crate) fn search_path_for_any_file_with_value(
    cmd: &str,
    path_value: &str,
    state: &InterpreterState,
) -> Option<String> {
    if cmd.is_empty() {
        return None;
    }

    if cmd.contains('/') {
        let resolved = resolve_path(&state.cwd, cmd);
        return state
            .fs
            .stat(Path::new(&resolved))
            .is_ok_and(|meta| matches!(meta.node_type, NodeType::File | NodeType::Symlink))
            .then(|| cmd.to_string());
    }

    for dir in path_value.split(':') {
        let candidate = if dir.is_empty() {
            format!("./{cmd}")
        } else {
            format!("{dir}/{cmd}")
        };
        let resolved = resolve_path(&state.cwd, &candidate);
        if state
            .fs
            .stat(Path::new(&resolved))
            .is_ok_and(|meta| matches!(meta.node_type, NodeType::File | NodeType::Symlink))
        {
            return Some(candidate);
        }
    }
    None
}

pub(crate) fn search_path_for_any_file(cmd: &str, state: &InterpreterState) -> Option<String> {
    let path_value = current_path(state);
    search_path_for_any_file_with_value(cmd, &path_value, state)
}

fn path_has_overlong_component(path: &str) -> bool {
    path.split('/').any(|component| component.len() > 255)
}

fn command_stub_target(script: &str) -> Option<&str> {
    script
        .lines()
        .find_map(|line| line.strip_prefix("# built-in: "))
        .map(str::trim)
        .filter(|name| !name.is_empty())
}

pub(crate) fn execute_registered_command_by_name(
    name: &str,
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
    extra_input_fds: Option<&std::collections::HashMap<i32, String>>,
) -> Result<ExecResult, RustBashError> {
    if let Some(cmd) = state.commands.get(name) {
        let mut env: std::collections::HashMap<String, String> = state
            .env
            .iter()
            .filter(|(_, v)| v.exported() && matches!(v.value, VariableValue::Scalar(_)))
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

        let vars_clone = state.env.clone();
        let fs = std::sync::Arc::clone(&state.fs);
        let cwd = state.cwd.clone();
        let limits = state.limits.clone();
        let network_policy = state.network_policy.clone();
        let binary_stdin = state.pipe_stdin_bytes.take();
        let exec_callback = crate::interpreter::walker::make_exec_callback(state);

        let ctx = crate::commands::CommandContext {
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

    Ok(ExecResult {
        stderr: format!("{name}: command not found\n"),
        exit_code: 127,
        ..ExecResult::default()
    })
}

pub(crate) fn execute_path_command(
    invocation_name: &str,
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
    extra_input_fds: Option<&std::collections::HashMap<i32, String>>,
) -> Result<ExecResult, RustBashError> {
    if path_has_overlong_component(invocation_name) {
        return Ok(ExecResult {
            stderr: format!("{invocation_name}: Permission denied\n"),
            exit_code: 126,
            ..ExecResult::default()
        });
    }

    let resolved = resolve_path(&state.cwd, invocation_name);
    let meta = match state.fs.stat(Path::new(&resolved)) {
        Ok(meta) => meta,
        Err(_) => {
            return Ok(ExecResult {
                stderr: format!("{invocation_name}: command not found\n"),
                exit_code: 127,
                ..ExecResult::default()
            });
        }
    };

    if meta.node_type == NodeType::Directory {
        return Ok(ExecResult {
            stderr: format!("{invocation_name}: Permission denied\n"),
            exit_code: 126,
            ..ExecResult::default()
        });
    }

    if meta.mode & 0o111 == 0 {
        return Ok(ExecResult {
            stderr: format!("{invocation_name}: Permission denied\n"),
            exit_code: 126,
            ..ExecResult::default()
        });
    }

    let bytes = match state.fs.read_file(Path::new(&resolved)) {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(ExecResult {
                stderr: format!("{invocation_name}: command not found\n"),
                exit_code: 127,
                ..ExecResult::default()
            });
        }
    };
    let script = String::from_utf8_lossy(&bytes).into_owned();

    if let Some(target) = command_stub_target(&script) {
        if let Some(help) = if args.first().map(|arg| arg.as_str()) == Some("--help") {
            check_help(target, state)
        } else {
            None
        } {
            return Ok(help);
        }
        if let Some(result) = execute_builtin(target, args, state, stdin, extra_input_fds)? {
            return Ok(result);
        }
        return execute_registered_command_by_name(target, args, state, stdin, extra_input_fds);
    }

    let program = match parse(&script) {
        Ok(program) => program,
        Err(err) => {
            return Ok(ExecResult {
                stderr: format!("{invocation_name}: {err}\n"),
                exit_code: 126,
                ..ExecResult::default()
            });
        }
    };

    run_in_subshell(
        state,
        &program,
        SubshellConfig {
            positional: args,
            shell_name_override: Some(invocation_name),
            source_override: Some(resolved),
            source_text_override: Some(script),
            invoked_with_c: false,
            shell_process: true,
        },
    )
}

fn is_executable_path(path: &str, state: &InterpreterState) -> bool {
    state.fs.stat(Path::new(path)).is_ok_and(|meta| {
        matches!(meta.node_type, NodeType::File | NodeType::Symlink) && meta.mode & 0o111 != 0
    })
}

// ── type ────────────────────────────────────────────────────────────

fn builtin_type(
    args: &[String],
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let mut t_flag = false;
    let mut a_flag = false;
    let mut p_flag = false;
    let mut big_p_flag = false;
    let mut f_flag = false;
    let mut names: Vec<&str> = Vec::new();

    for arg in args {
        if arg.starts_with('-') && names.is_empty() {
            for c in arg[1..].chars() {
                match c {
                    't' => t_flag = true,
                    'a' => a_flag = true,
                    'p' => p_flag = true,
                    'P' => big_p_flag = true,
                    'f' => f_flag = true,
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
        let paths = type_search_paths(name, state);

        // -P: only search PATH (skip builtins, functions, aliases, keywords)
        if big_p_flag {
            if paths.is_empty() {
                exit_code = 1;
            } else {
                for path in &paths {
                    stdout.push_str(&format!("{path}\n"));
                    found = true;
                    if !a_flag {
                        break;
                    }
                }
            }
            if !found {
                exit_code = 1;
            }
            continue;
        }

        // Check alias (-f only suppresses function lookup, not aliases)
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

        // Check keyword
        if is_shell_keyword(name) {
            if t_flag {
                stdout.push_str("keyword\n");
            } else if !p_flag {
                stdout.push_str(&format!("{name} is a shell keyword\n"));
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        // Check function (skip if -f flag)
        if !f_flag && let Some(func) = state.functions.get(*name) {
            if t_flag {
                stdout.push_str("function\n");
            } else if !p_flag {
                stdout.push_str(&format!("{name} is a function\n"));
                // Print function body
                let body_str = format_function_body(name, func);
                stdout.push_str(&body_str);
                stdout.push('\n');
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        if is_special_builtin(name) {
            if t_flag {
                stdout.push_str("builtin\n");
            } else if !p_flag {
                stdout.push_str(&format!("{name} is a shell builtin\n"));
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        // Check regular builtin
        if !is_special_builtin(name)
            && (is_builtin(name) || INTROSPECTION_ONLY_BUILTINS.contains(name))
        {
            if t_flag {
                stdout.push_str("builtin\n");
            } else if !p_flag {
                stdout.push_str(&format!("{name} is a shell builtin\n"));
            }
            found = true;
            if !a_flag {
                continue;
            }
        }

        // Check PATH — with -a, list all matches
        if t_flag && paths.is_empty() {
            if search_path_for_any_file(name, state).is_some() {
                stdout.push_str("file\n");
                found = true;
            }
            if found && !a_flag {
                continue;
            }
        }
        for path in &paths {
            if t_flag {
                stdout.push_str("file\n");
            } else if p_flag {
                stdout.push_str(&format!("{path}\n"));
            } else {
                stdout.push_str(&format!("{name} is {path}\n"));
            }
            found = true;
            if !a_flag {
                break;
            }
        }

        if !found {
            // -t: no stderr for not-found (just set exit code)
            if !t_flag {
                stderr.push_str(&format!("type: {name}: not found\n"));
            }
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

fn type_search_paths(name: &str, state: &InterpreterState) -> Vec<String> {
    search_path_all(name, state)
        .into_iter()
        .filter(|path| {
            !type_ignores_shell_only_stub(name) || !is_synthetic_command_stub(path, name, state)
        })
        .collect()
}

fn type_ignores_shell_only_stub(name: &str) -> bool {
    matches!(
        name,
        "." | ":"
            | "alias"
            | "builtin"
            | "cd"
            | "command"
            | "declare"
            | "eval"
            | "exec"
            | "exit"
            | "export"
            | "local"
            | "readonly"
            | "return"
            | "set"
            | "shift"
            | "source"
            | "trap"
            | "type"
            | "typeset"
            | "unalias"
            | "unset"
    )
}

fn is_synthetic_command_stub(path: &str, name: &str, state: &InterpreterState) -> bool {
    state
        .fs
        .read_file(Path::new(path))
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|script| command_stub_target(&script).map(str::to_string))
        .is_some_and(|target| target == name)
}

/// Format a function body for `type` output, mimicking bash's format.
fn format_function_body(name: &str, func: &crate::interpreter::FunctionDef) -> String {
    if let Some(start) = func.definition.find('{')
        && let Some(end) = func.definition.rfind('}')
        && end > start
    {
        let body = &func.definition[start + 1..end];
        let statements: Vec<String> = body
            .lines()
            .flat_map(|line| line.split(';'))
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToString::to_string)
            .collect();
        let mut formatted = format!("{name} () \n{{ \n");
        for statement in statements {
            formatted.push_str("    ");
            formatted.push_str(&statement);
            formatted.push('\n');
        }
        formatted.push('}');
        return formatted;
    }

    format!("{name} () \n{{ \n}}")
}

/// Search all PATH entries for a command and return all matches.
fn search_path_all(cmd: &str, state: &InterpreterState) -> Vec<String> {
    if cmd.contains('/') {
        let resolved = resolve_path(&state.cwd, cmd);
        return if is_executable_path(&resolved, state) {
            vec![cmd.to_string()]
        } else {
            Vec::new()
        };
    }

    let path_var = state
        .env
        .get("PATH")
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_else(|| "/usr/bin:/bin".to_string());
    let mut results = Vec::new();
    for dir in path_var.split(':') {
        let candidate = if dir.is_empty() {
            format!("./{cmd}")
        } else {
            format!("{dir}/{cmd}")
        };
        let resolved = resolve_path(&state.cwd, &candidate);
        if is_executable_path(&resolved, state) {
            results.push(candidate);
        }
    }
    results
}

// ── command ─────────────────────────────────────────────────────────

fn builtin_command(
    args: &[String],
    state: &mut InterpreterState,
    stdin: &str,
    extra_input_fds: Option<&std::collections::HashMap<i32, String>>,
) -> Result<ExecResult, RustBashError> {
    let mut v_flag = false;
    let mut big_v_flag = false;
    let mut p_flag = false;
    let mut cmd_start = 0;

    // Parse flags
    for (i, arg) in args.iter().enumerate() {
        if arg == "--" && cmd_start == i {
            cmd_start = i + 1;
            break;
        }
        if arg.starts_with('-') && cmd_start == i {
            let mut consumed = true;
            for c in arg[1..].chars() {
                match c {
                    'v' => v_flag = true,
                    'V' => big_v_flag = true,
                    'p' => p_flag = true,
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

    // command -v: print how name would be resolved
    if v_flag || big_v_flag {
        let path_override = p_flag.then_some(crate::interpreter::DEFAULT_PATH);
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut any_found = false;
        for name in remaining {
            let result = if v_flag {
                command_v(name, state, path_override)?
            } else {
                command_big_v(name, state, path_override)?
            };
            stdout.push_str(&result.stdout);
            stderr.push_str(&result.stderr);
            if result.exit_code != 0 {
                continue;
            }
            any_found = true;
        }
        return Ok(ExecResult {
            stdout,
            stderr,
            exit_code: if any_found { 0 } else { 1 },
            ..ExecResult::default()
        });
    }

    // command name [args]: run bypassing functions — only builtins and commands
    let name = &remaining[0];
    let cmd_args = &remaining[1..];
    let cmd_args_owned: Vec<String> = cmd_args.to_vec();

    // --help interception (consistent with dispatch_command)
    if cmd_args_owned.first().map(|a| a.as_str()) == Some("--help")
        && let Some(help) = check_help(name, state)
    {
        return Ok(help);
    }

    // Try builtin first
    if let Some(result) = execute_builtin(name, &cmd_args_owned, state, stdin, extra_input_fds)? {
        return Ok(result);
    }

    if name.contains('/') {
        return execute_path_command(name, &cmd_args_owned, state, stdin, extra_input_fds);
    }

    let path_value = if p_flag {
        crate::interpreter::DEFAULT_PATH.to_string()
    } else {
        current_path(state)
    };

    if let Some(path) = search_path_with_value(name, &path_value, state) {
        return execute_path_command(&path, &cmd_args_owned, state, stdin, extra_input_fds);
    }

    // Not found
    Ok(ExecResult {
        stderr: format!("{name}: command not found\n"),
        exit_code: 127,
        ..ExecResult::default()
    })
}

/// Shell keywords recognized by type/command -v
const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "time", "{", "}", "!", "[[", "]]", "coproc",
];

fn is_shell_keyword(name: &str) -> bool {
    SHELL_KEYWORDS.contains(&name)
}

fn command_v(
    name: &str,
    state: &InterpreterState,
    path_override: Option<&str>,
) -> Result<ExecResult, RustBashError> {
    // Keyword
    if is_shell_keyword(name) {
        return Ok(ExecResult {
            stdout: format!("{name}\n"),
            ..ExecResult::default()
        });
    }

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

    if is_special_builtin(name) {
        return Ok(ExecResult {
            stdout: format!("{name}\n"),
            ..ExecResult::default()
        });
    }

    // Regular builtin
    if is_builtin(name) || INTROSPECTION_ONLY_BUILTINS.contains(&name) {
        return Ok(ExecResult {
            stdout: format!("{name}\n"),
            ..ExecResult::default()
        });
    }

    let path = path_override
        .and_then(|value| search_path_with_value(name, value, state))
        .or_else(|| search_path(name, state));
    if let Some(path) = path {
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

fn command_big_v(
    name: &str,
    state: &InterpreterState,
    path_override: Option<&str>,
) -> Result<ExecResult, RustBashError> {
    if is_shell_keyword(name) {
        return Ok(ExecResult {
            stdout: format!("{name} is a shell keyword\n"),
            ..ExecResult::default()
        });
    }

    if let Some(expansion) = state.aliases.get(name) {
        return Ok(ExecResult {
            stdout: format!("{name} is aliased to `{expansion}'\n"),
            ..ExecResult::default()
        });
    }

    if let Some(func) = state.functions.get(name) {
        return Ok(ExecResult {
            stdout: format!(
                "{name} is a function\n{}\n",
                format_function_body(name, func)
            ),
            ..ExecResult::default()
        });
    }

    if is_special_builtin(name) {
        return Ok(ExecResult {
            stdout: format!("{name} is a shell builtin\n"),
            ..ExecResult::default()
        });
    }

    if is_builtin(name) || INTROSPECTION_ONLY_BUILTINS.contains(&name) {
        return Ok(ExecResult {
            stdout: format!("{name} is a shell builtin\n"),
            ..ExecResult::default()
        });
    }

    let path = path_override
        .and_then(|value| search_path_with_value(name, value, state))
        .or_else(|| search_path(name, state));
    if let Some(path) = path {
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
    extra_input_fds: Option<&std::collections::HashMap<i32, String>>,
) -> Result<ExecResult, RustBashError> {
    let args = if args.first().map(|arg| arg.as_str()) == Some("--") {
        &args[1..]
    } else {
        args
    };

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

    if matches!(name.as_str(), "declare" | "typeset")
        && sub_args
            .iter()
            .any(|arg| arg.contains("=(") || arg.contains("+=("))
    {
        return Ok(ExecResult {
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    // Try shell builtins first
    if let Some(result) = execute_builtin(name, &sub_args, state, stdin, extra_input_fds)? {
        return Ok(result);
    }

    // A handful of shell builtins are implemented as registered commands.
    if INTROSPECTION_ONLY_BUILTINS.contains(&name.as_str()) {
        return execute_registered_command_by_name(name, &sub_args, state, stdin, extra_input_fds);
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
    let invalid_var_name = !is_valid_var_name(var_name);

    // If extra args provided, use them; otherwise use positional params
    let option_args: Vec<String> = if args.len() > 2 {
        args[2..].to_vec()
    } else {
        state.positional_params.clone()
    };
    let args_signature = option_args.join("\u{1f}");
    let previous_args_signature =
        std::mem::replace(&mut state.getopts_args_signature, args_signature.clone());

    // Loop instead of recursion: advance to the next argument when the
    // sub-position within bundled flags has been exhausted.
    loop {
        let mut optind: usize = state
            .env
            .get("OPTIND")
            .and_then(|v| v.value.as_scalar().parse().ok())
            .unwrap_or(1);
        if optind == 0 {
            optind = 1;
        }

        let reset_due_to_new_args =
            previous_args_signature != args_signature && optind > option_args.len();
        if reset_due_to_new_args {
            set_variable(state, "OPTIND", "1".to_string())?;
            state.getopts_subpos = 0;
            optind = 1;
        }

        let idx = optind.saturating_sub(1);

        if idx >= option_args.len() {
            state.getopts_subpos = 0;
            if !invalid_var_name {
                set_variable(state, var_name, "?".to_string())?;
            }
            if state.env.contains_key("OPTARG") {
                set_variable(state, "OPTARG", String::new())?;
            } else {
                state.env.remove("OPTARG");
            }
            return Ok(ExecResult {
                exit_code: 1,
                ..ExecResult::default()
            });
        }

        let current_arg = &option_args[idx];

        if !current_arg.starts_with('-') || current_arg == "-" || current_arg == "--" {
            state.getopts_subpos = 0;
            if !invalid_var_name {
                set_variable(state, var_name, "?".to_string())?;
            }
            if state.env.contains_key("OPTARG") {
                set_variable(state, "OPTARG", String::new())?;
            } else {
                state.env.remove("OPTARG");
            }
            if current_arg == "--" {
                set_variable(state, "OPTIND", (optind + 1).to_string())?;
            }
            return Ok(ExecResult {
                exit_code: 1,
                ..ExecResult::default()
            });
        }

        let opt_chars: Vec<char> = current_arg[1..].chars().collect();
        let sub_pos = state.getopts_subpos;

        if sub_pos >= opt_chars.len() {
            // Advance to next argument and retry (loop, not recurse).
            state.getopts_subpos = 0;
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
                    state.getopts_subpos = 0;
                    set_variable(state, "OPTIND", (optind + 1).to_string())?;
                } else if idx + 1 < option_args.len() {
                    set_variable(state, "OPTARG", option_args[idx + 1].clone())?;
                    state.getopts_subpos = 0;
                    set_variable(state, "OPTIND", (optind + 2).to_string())?;
                } else {
                    // Missing argument
                    state.getopts_subpos = 0;
                    set_variable(state, "OPTIND", (optind + 1).to_string())?;
                    if reset_due_to_new_args {
                        if state.env.contains_key("OPTARG") {
                            set_variable(state, "OPTARG", String::new())?;
                        } else {
                            state.env.remove("OPTARG");
                        }
                        if !invalid_var_name {
                            set_variable(state, var_name, "?".to_string())?;
                        }
                        return Ok(ExecResult {
                            exit_code: 1,
                            ..ExecResult::default()
                        });
                    }
                    if silent {
                        if !invalid_var_name {
                            set_variable(state, var_name, ":".to_string())?;
                        }
                        set_variable(state, "OPTARG", opt_char.to_string())?;
                        return Ok(ExecResult {
                            exit_code: if invalid_var_name { 1 } else { 0 },
                            ..ExecResult::default()
                        });
                    }
                    if !invalid_var_name {
                        set_variable(state, var_name, "?".to_string())?;
                    }
                    return Ok(ExecResult {
                        stderr: format!("getopts: option requires an argument -- '{opt_char}'\n"),
                        exit_code: if invalid_var_name { 1 } else { 0 },
                        ..ExecResult::default()
                    });
                }
            } else {
                if state.env.contains_key("OPTARG") {
                    set_variable(state, "OPTARG", String::new())?;
                } else {
                    state.env.remove("OPTARG");
                }
                if sub_pos + 1 < opt_chars.len() {
                    state.getopts_subpos = sub_pos + 1;
                } else {
                    state.getopts_subpos = 0;
                    set_variable(state, "OPTIND", (optind + 1).to_string())?;
                }
            }
            if !invalid_var_name {
                set_variable(state, var_name, opt_char.to_string())?;
            }
            return Ok(ExecResult {
                exit_code: if invalid_var_name { 1 } else { 0 },
                ..ExecResult::default()
            });
        }

        // Invalid option
        if silent {
            if !invalid_var_name {
                set_variable(state, var_name, "?".to_string())?;
            }
            set_variable(state, "OPTARG", opt_char.to_string())?;
        } else {
            if !invalid_var_name {
                set_variable(state, var_name, "?".to_string())?;
            }
            set_variable(state, "OPTARG", String::new())?;
        }
        if sub_pos + 1 < opt_chars.len() {
            state.getopts_subpos = sub_pos + 1;
        } else {
            state.getopts_subpos = 0;
            set_variable(state, "OPTIND", (optind + 1).to_string())?;
        }
        let stderr = if silent {
            String::new()
        } else {
            format!("getopts: illegal option -- '{opt_char}'\n")
        };
        return Ok(ExecResult {
            stderr,
            exit_code: if invalid_var_name { 1 } else { 0 },
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
    let mut origin: usize = 0;
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
                    'C' | 'c' | 'u' => {
                        // -C callback, -c quantum, -u fd — skip values
                        let rest: String = chars.collect();
                        if rest.is_empty() {
                            i += 1; // skip the argument value
                        }
                        break;
                    }
                    'O' => {
                        let rest: String = chars.collect();
                        let origin_str = if rest.is_empty() {
                            i += 1;
                            if i < args.len() {
                                args[i].as_str()
                            } else {
                                "0"
                            }
                        } else {
                            &rest
                        };
                        origin = origin_str.parse().unwrap_or(0);
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
        stdin.split_terminator('\0').collect()
    } else {
        split_keeping_delimiter(stdin, delimiter)
    };

    let mut map = if origin == 0 {
        std::collections::BTreeMap::new()
    } else {
        match state.env.get(array_name.as_str()) {
            Some(var) => match &var.value {
                VariableValue::IndexedArray(existing) => existing.clone(),
                _ => std::collections::BTreeMap::new(),
            },
            None => std::collections::BTreeMap::new(),
        }
    };
    let mut count = 0usize;

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
        map.insert(origin + count, value);
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

    // Reject multiple arguments
    let mut positional = Vec::new();
    let mut saw_dashdash = false;
    for arg in args {
        if saw_dashdash {
            positional.push(arg);
        } else if arg == "--" {
            saw_dashdash = true;
        } else if arg == "-" {
            positional.push(arg);
        } else if arg.starts_with('-')
            && !arg[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
            && !arg.starts_with('+')
        {
            // Invalid flag like -z
            return Ok(ExecResult {
                stderr: format!("pushd: {arg}: invalid option\n"),
                exit_code: 2,
                ..ExecResult::default()
            });
        } else {
            positional.push(arg);
        }
    }

    if positional.len() > 1 {
        return Ok(ExecResult {
            stderr: "pushd: too many arguments\n".to_string(),
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    let arg = positional.first().copied().unwrap_or(&args[0]);

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
    if !args.is_empty() {
        let arg = &args[0];

        // `--` terminates options
        if arg == "--" {
            if state.dir_stack.is_empty() {
                return Ok(ExecResult {
                    stderr: "popd: directory stack empty\n".to_string(),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
            // popd -- is just popd (default behavior)
            let top = state.dir_stack.remove(0);
            let result = builtin_cd(std::slice::from_ref(&top), state)?;
            if result.exit_code != 0 {
                state.dir_stack.insert(0, top);
                return Ok(result);
            }
            return Ok(dirs_output(state));
        }

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

        // Invalid argument (not +N or -N)
        return Ok(ExecResult {
            stderr: format!("popd: {arg}: invalid argument\n"),
            exit_code: 2,
            ..ExecResult::default()
        });
    }

    if state.dir_stack.is_empty() {
        return Ok(ExecResult {
            stderr: "popd: directory stack empty\n".to_string(),
            exit_code: 1,
            ..ExecResult::default()
        });
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
            if flags.is_empty() {
                // bare "-" is not a valid argument
                return Ok(ExecResult {
                    stderr: "dirs: -: invalid option\n".to_string(),
                    exit_code: 1,
                    ..ExecResult::default()
                });
            }
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
        } else if arg.starts_with('+') {
            // +N is ok (not yet handled but valid syntax)
            continue;
        } else {
            // Non-flag arguments are not accepted
            return Ok(ExecResult {
                stderr: format!("dirs: {arg}: invalid argument\n"),
                exit_code: 1,
                ..ExecResult::default()
            });
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
            stdout.push_str(&format!(" {i}  {display}\n"));
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
        stdout.push_str("hits\tcommand\n");
        let mut entries: Vec<(&String, &String)> = state.command_hash.iter().collect();
        entries.sort_by_key(|(k, _)| k.as_str());
        for (_name, path) in entries {
            stdout.push_str(&format!("   1\t{path}\n"));
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
        let vname = &remaining_args[1];
        // Validate variable name: must be identifier or identifier[subscript]
        let base_name = vname.split('[').next().unwrap_or(vname);
        let valid_base = !base_name.is_empty()
            && base_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !base_name.starts_with(|c: char| c.is_ascii_digit());
        let valid_subscript = if let Some(bracket_pos) = vname.find('[') {
            vname.ends_with(']') && bracket_pos + 1 < vname.len() - 1
        } else {
            true
        };
        if !valid_base || !valid_subscript {
            return Ok(ExecResult {
                stderr: format!("printf: `{vname}': not a valid identifier\n"),
                exit_code: 2,
                ..ExecResult::default()
            });
        }
        var_name = Some(vname.clone());
        remaining_args = &remaining_args[2..];
    }

    // Skip -- end-of-options marker
    if !remaining_args.is_empty() && remaining_args[0] == "--" {
        remaining_args = &remaining_args[1..];
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
    let result = crate::commands::text::run_printf_format(
        format_str,
        arguments,
        crate::commands::text::PrintfContext {
            shell_vars: Some(&state.env),
            env: None,
        },
    );
    let stdout_bytes = crate::shell_bytes::contains_markers(&result.stdout)
        .then(|| crate::shell_bytes::encode_shell_string(&result.stdout));

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
            stdout_bytes,
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
        return run_in_subshell(
            state,
            &program,
            SubshellConfig {
                positional: &[],
                shell_name_override: None,
                source_override: None,
                source_text_override: Some(stdin.to_string()),
                invoked_with_c: false,
                shell_process: true,
            },
        );
    }

    let saved_shell_opts = state.shell_opts.clone();
    let saved_shopt_opts = state.shopt_opts.clone();
    let saved_interactive_shell = state.interactive_shell;

    let result = (|| {
        // Phase 1: Parse all invocation options.  `-c` sets a flag rather
        // than immediately consuming the next argument so that subsequent
        // flags (e.g. `-x`, `-e`) are still processed, matching bash.
        let mut command_mode = false;
        let mut i = 0;

        'opts: while i < args.len() {
            let arg = &args[i];

            // `--` and bare `-` terminate option processing.
            if arg == "--" || arg == "-" {
                i += 1;
                break;
            }

            // Long options (must be checked before the generic `-…` branch
            // so that e.g. `--login` is not parsed char-by-char).
            if arg.starts_with("--") {
                match arg.as_str() {
                    "--rcfile" | "--init-file" => {
                        i += 1;
                        if i >= args.len() {
                            return Ok(ExecResult {
                                stderr: format!("sh: {arg}: option requires an argument\n"),
                                exit_code: 2,
                                ..ExecResult::default()
                            });
                        }
                        i += 1;
                        continue;
                    }
                    "--norc" | "--noprofile" | "--login" => {
                        i += 1;
                        continue;
                    }
                    _ => {
                        // Unknown long option — stop option processing so
                        // it becomes the first operand (script / -c string).
                        // (Short flags use an error instead; the asymmetry
                        // mirrors bash, which rejects `-z` but treats
                        // `--unknown` as a filename operand.)
                        break;
                    }
                }
            }

            // Short option groups: `-xef`, `+o name`, `-oo a b`, `-c`, etc.
            if (arg.starts_with('-') || arg.starts_with('+')) && arg.len() > 1 {
                let enable = arg.starts_with('-');
                let mut extra_consumed: usize = 0;

                for ch in arg[1..].chars() {
                    match ch {
                        'c' => command_mode = true,
                        // Accepted invocation-only flags (silently consumed).
                        'l' | 's' | 'r' | 'D' | 'h' | 'b' | 'B' | 'H' | 'P' | 'T' => {}
                        'i' => {
                            if enable {
                                state.interactive_shell = true;
                                state.shell_opts.emacs_mode = true;
                                state.shell_opts.vi_mode = false;
                            } else {
                                state.interactive_shell = false;
                            }
                        }
                        'a' | 'e' | 'f' | 'n' | 'u' | 'v' | 'x' | 'C' => {
                            apply_option_char(ch, enable, state);
                        }
                        'o' | 'O' => {
                            let name_idx = i + 1 + extra_consumed;
                            if name_idx >= args.len() {
                                let flag_str = if enable {
                                    format!("-{ch}")
                                } else {
                                    format!("+{ch}")
                                };
                                return Ok(ExecResult {
                                    stderr: format!(
                                        "sh: {flag_str}: option requires an argument\n"
                                    ),
                                    exit_code: 2,
                                    ..ExecResult::default()
                                });
                            }
                            let name = &args[name_idx];
                            let ok = if ch == 'O' {
                                set_shopt(state, name, enable)
                            } else if get_set_option(name, state).is_some() {
                                apply_option_name(name, enable, state);
                                true
                            } else {
                                false
                            };
                            if !ok {
                                return Ok(ExecResult {
                                    stderr: format!("sh: {name}: invalid option name\n"),
                                    exit_code: 2,
                                    ..ExecResult::default()
                                });
                            }
                            extra_consumed += 1;
                        }
                        _ => {
                            let prefix = if enable { '-' } else { '+' };
                            return Ok(ExecResult {
                                stderr: format!("sh: {prefix}{ch}: invalid option\n"),
                                exit_code: 2,
                                ..ExecResult::default()
                            });
                        }
                    }
                }

                i += 1 + extra_consumed;
                continue 'opts;
            }

            // Non-option argument — stop processing.
            break;
        }

        // Phase 2: Dispatch on the remaining (non-option) arguments.
        let remaining = &args[i..];

        if command_mode {
            if remaining.is_empty() {
                return Ok(ExecResult {
                    stderr: "sh: -c: option requires an argument\n".into(),
                    exit_code: 2,
                    ..ExecResult::default()
                });
            }
            let cmd = &remaining[0];
            let extra = &remaining[1..];
            let shell_name_override = extra.first().map(|s| s.as_str());
            let positional: Vec<String> = if extra.len() > 1 {
                extra[1..].to_vec()
            } else {
                Vec::new()
            };
            let program = parse(cmd)?;
            return run_in_subshell(
                state,
                &program,
                SubshellConfig {
                    positional: &positional,
                    shell_name_override,
                    source_override: None,
                    source_text_override: Some(cmd.to_string()),
                    invoked_with_c: true,
                    shell_process: true,
                },
            );
        }

        // Script-file mode.
        if let Some(script_arg) = remaining.first() {
            let path = crate::interpreter::builtins::resolve_path(&state.cwd, script_arg);
            let path_buf = std::path::PathBuf::from(&path);
            match state.fs.read_file(&path_buf) {
                Ok(bytes) => {
                    let script = String::from_utf8_lossy(&bytes).to_string();
                    let positional = remaining[1..].to_vec();
                    let program = parse(&script)?;
                    return run_in_subshell(
                        state,
                        &program,
                        SubshellConfig {
                            positional: &positional,
                            shell_name_override: Some(script_arg.as_str()),
                            source_override: Some(path),
                            source_text_override: Some(script),
                            invoked_with_c: false,
                            shell_process: true,
                        },
                    );
                }
                Err(e) => {
                    return Ok(ExecResult {
                        stderr: format!("sh: {}: {}\n", script_arg, e),
                        exit_code: 127,
                        ..ExecResult::default()
                    });
                }
            }
        }

        // No operands — read from stdin.
        if !stdin.is_empty() {
            let program = parse(stdin)?;
            return run_in_subshell(
                state,
                &program,
                SubshellConfig {
                    positional: &[],
                    shell_name_override: None,
                    source_override: None,
                    source_text_override: Some(stdin.to_string()),
                    invoked_with_c: false,
                    shell_process: true,
                },
            );
        }

        Ok(ExecResult::default())
    })();

    state.shell_opts = saved_shell_opts;
    state.shopt_opts = saved_shopt_opts;
    state.interactive_shell = saved_interactive_shell;

    result
}

/// Execute a parsed program in an isolated subshell, returning only
/// stdout/stderr/exit_code. Mirrors `execute_subshell` in walker.rs.
struct SubshellConfig<'a> {
    positional: &'a [String],
    shell_name_override: Option<&'a str>,
    source_override: Option<String>,
    source_text_override: Option<String>,
    invoked_with_c: bool,
    shell_process: bool,
}

fn run_in_subshell(
    state: &mut InterpreterState,
    program: &brush_parser::ast::Program,
    config: SubshellConfig<'_>,
) -> Result<ExecResult, RustBashError> {
    use std::collections::HashMap;
    // Track whether this is a script file invocation (not -c) for FUNCNAME "main" frame.
    // Use the shell_name_override (original path argument) for relative paths.
    let script_source = if !config.invoked_with_c && config.source_override.is_some() {
        config
            .shell_name_override
            .map(|s| s.to_string())
            .or_else(|| config.source_override.clone())
    } else {
        None
    };
    let cloned_fs = state.fs.deep_clone();
    let child_pid = next_child_pid(state);
    let env = if config.shell_process {
        state
            .env
            .iter()
            .filter(|(_, var)| var.exported())
            .map(|(name, var)| (name.clone(), var.clone()))
            .collect()
    } else {
        state.env.clone()
    };
    let mut sub_state = InterpreterState {
        fs: cloned_fs,
        env,
        cwd: state.cwd.clone(),
        functions: if config.shell_process {
            HashMap::new()
        } else {
            state.functions.clone()
        },
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
        abort_command_list: false,
        loop_depth: 0,
        control_flow: None,
        positional_params: if config.positional.is_empty() {
            state.positional_params.clone()
        } else {
            config.positional.to_vec()
        },
        shell_name: config
            .shell_name_override
            .map(|s| s.to_string())
            .unwrap_or_else(|| state.shell_name.clone()),
        shell_pid: state.shell_pid,
        bash_pid: child_pid,
        parent_pid: state.bash_pid,
        next_process_id: state.next_process_id,
        last_background_pid: None,
        last_background_status: None,
        interactive_shell: state.interactive_shell,
        invoked_with_c: config.invoked_with_c,
        random_seed: state.random_seed,
        local_scopes: Vec::new(),
        temp_binding_scopes: Vec::new(),
        in_function_depth: 0,
        source_depth: if config.shell_process {
            0
        } else {
            state.source_depth
        },
        getopts_subpos: if config.shell_process {
            0
        } else {
            state.getopts_subpos
        },
        getopts_args_signature: if config.shell_process {
            String::new()
        } else {
            state.getopts_args_signature.clone()
        },
        traps: state.traps.clone(),
        in_trap: false,
        errexit_suppressed: if config.shell_process {
            0
        } else {
            state.errexit_suppressed
        },
        errexit_bang_suppressed: if config.shell_process {
            0
        } else {
            state.errexit_bang_suppressed
        },
        stdin_offset: 0,
        current_stdin_persistent_fd: None,
        dir_stack: state.dir_stack.clone(),
        command_hash: state.command_hash.clone(),
        aliases: if config.shell_process {
            HashMap::new()
        } else {
            state.aliases.clone()
        },
        current_lineno: state.current_lineno,
        current_source: config
            .shell_name_override
            .map(|s| s.to_string())
            .or(config.source_override)
            .unwrap_or_else(|| state.current_source.clone()),
        current_source_text: config
            .source_text_override
            .unwrap_or_else(|| state.current_source_text.clone()),
        last_verbose_line: if config.shell_process {
            0
        } else {
            state.last_verbose_line
        },
        shell_start_time: state.shell_start_time,
        last_argument: state.last_argument.clone(),
        call_stack: state.call_stack.clone(),
        machtype: state.machtype.clone(),
        hosttype: state.hosttype.clone(),
        persistent_fds: state.persistent_fds.clone(),
        persistent_fd_offsets: state.persistent_fd_offsets.clone(),
        next_auto_fd: state.next_auto_fd,
        proc_sub_counter: state.proc_sub_counter,
        proc_sub_prealloc: HashMap::new(),
        pipe_stdin_bytes: None,
        pending_cmdsub_stderr: String::new(),
        pending_test_stderr: String::new(),
        fatal_expansion_error: false,
        last_command_had_error: false,
        last_status_immune_to_errexit: false,
        script_source,
    };
    ensure_nested_shell_startup_vars(&mut sub_state);

    let result = execute_program(program, &mut sub_state).map(|mut result| {
        if sub_state.fatal_expansion_error {
            result.exit_code = 127;
        }
        result
    });

    // Fold shared counters back into parent
    state.counters.command_count = sub_state.counters.command_count;
    state.counters.output_size = sub_state.counters.output_size;
    fold_child_process_state(state, &sub_state);
    state.last_verbose_line = state.last_verbose_line.max(sub_state.last_verbose_line);

    result
}

// ── help builtin ────────────────────────────────────────────────────

fn builtin_help(args: &[String], state: &InterpreterState) -> Result<ExecResult, RustBashError> {
    let args = if args.first().map(|arg| arg.as_str()) == Some("--") {
        &args[1..]
    } else {
        args
    };

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

fn builtin_history(
    args: &[String],
    _state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    if args.is_empty() {
        return Ok(ExecResult::default());
    }

    if args.len() == 1 {
        let arg = &args[0];
        let is_numeric = !arg.is_empty() && arg.chars().all(|ch| ch.is_ascii_digit());
        let is_plus_numeric = arg
            .strip_prefix('+')
            .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()));

        if is_numeric || is_plus_numeric {
            return Ok(ExecResult::default());
        }

        if arg.starts_with('-') {
            return Ok(ExecResult {
                stderr: format!("history: {arg}: invalid option\n"),
                exit_code: 2,
                ..ExecResult::default()
            });
        }

        return Ok(ExecResult {
            stderr: format!("history: {arg}: numeric argument required\n"),
            exit_code: 1,
            ..ExecResult::default()
        });
    }

    Ok(ExecResult {
        stderr: "history: too many arguments\n".to_string(),
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
            abort_command_list: false,
            loop_depth: 0,
            control_flow: None,
            positional_params: Vec::new(),
            shell_name: "rust-bash".to_string(),
            shell_pid: 1000,
            bash_pid: 1000,
            parent_pid: 1,
            next_process_id: 1001,
            last_background_pid: None,
            last_background_status: None,
            interactive_shell: false,
            invoked_with_c: false,
            random_seed: 42,
            local_scopes: Vec::new(),
            temp_binding_scopes: Vec::new(),
            in_function_depth: 0,
            source_depth: 0,
            getopts_subpos: 0,
            getopts_args_signature: String::new(),
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
            errexit_bang_suppressed: 0,
            stdin_offset: 0,
            current_stdin_persistent_fd: None,
            dir_stack: Vec::new(),
            command_hash: HashMap::new(),
            aliases: HashMap::new(),
            current_lineno: 0,
            current_source: "main".to_string(),
            current_source_text: String::new(),
            last_verbose_line: 0,
            shell_start_time: Instant::now(),
            last_argument: String::new(),
            call_stack: Vec::new(),
            machtype: "x86_64-pc-linux-gnu".to_string(),
            hosttype: "x86_64".to_string(),
            persistent_fds: HashMap::new(),
            persistent_fd_offsets: HashMap::new(),
            next_auto_fd: 10,
            proc_sub_counter: 0,
            proc_sub_prealloc: HashMap::new(),
            pipe_stdin_bytes: None,
            pending_cmdsub_stderr: String::new(),
            pending_test_stderr: String::new(),
            fatal_expansion_error: false,
            last_command_had_error: false,
            last_status_immune_to_errexit: false,
            script_source: None,
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
    fn sh_reads_stdin_when_only_flags_are_given() {
        let mut state = make_state();
        let result = builtin_sh(&["-i".to_string()], &mut state, "printf '%s\\n' \"$0\"").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "rust-bash\n");
    }

    #[test]
    fn sh_script_uses_invoked_name_for_dollar_zero() {
        let mut state = make_state();
        state
            .fs
            .write_file(Path::new("/script.sh"), b"printf '%s\\n' \"$0\"")
            .unwrap();
        let result = builtin_sh(&["script.sh".to_string()], &mut state, "").unwrap();
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "script.sh\n");
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
        builtin_read(&["NAME".to_string()], &mut state, "hello world\n", None).unwrap();
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
            None,
        )
        .unwrap();
        assert_eq!(state.env.get("A").unwrap().value.as_scalar(), "one");
        assert_eq!(state.env.get("B").unwrap().value.as_scalar(), "two three");
    }

    #[test]
    fn read_reply_default() {
        let mut state = make_state();
        builtin_read(&[], &mut state, "test input\n", None).unwrap();
        assert_eq!(
            state.env.get("REPLY").unwrap().value.as_scalar(),
            "test input"
        );
    }

    #[test]
    fn read_eof_returns_1() {
        let mut state = make_state();
        let result = builtin_read(&["VAR".to_string()], &mut state, "", None).unwrap();
        assert_eq!(result.exit_code, 1);
    }

    #[test]
    fn read_into_array() {
        let mut state = make_state();
        builtin_read(
            &["-r".to_string(), "-a".to_string(), "arr".to_string()],
            &mut state,
            "a b c\n",
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
            None,
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
