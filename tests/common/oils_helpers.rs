//! Oils spec-test helper commands.
//!
//! These are Rust reimplementations of the small Python scripts that the Oils
//! test harness uses (`argv.py`, `printenv.py`, `stdout_stderr.py`) plus a
//! minimal `python2 -c` interpreter.  They live in the test crate — not the
//! library — so the production binary stays clean.

use rust_bash::{CommandContext, CommandMeta, CommandResult, VirtualCommand};

// ---------------------------------------------------------------------------
// argv.py
// ---------------------------------------------------------------------------

/// Oils test helper: `argv.py` prints arguments as a Python list.
///
/// Equivalent to `python2 -c 'import sys; print(sys.argv[1:])'`.
pub struct ArgvPyCommand;

static ARGV_PY_META: CommandMeta = CommandMeta {
    name: "argv.py",
    synopsis: "argv.py [arg ...]",
    description: "Print arguments as a Python list (Oils test helper).",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for ArgvPyCommand {
    fn name(&self) -> &str {
        "argv.py"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&ARGV_PY_META)
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let parts: Vec<String> = args.iter().map(|a| python_repr_string(a)).collect();
        CommandResult {
            stdout: format!("[{}]\n", parts.join(", ")),
            ..Default::default()
        }
    }
}

/// Produce a Python-style repr of a string, matching Python 2 behavior.
fn python_repr_string(s: &str) -> String {
    let bytes = s.as_bytes();
    let quote = if bytes.contains(&b'\'') && !bytes.contains(&b'"') {
        '"'
    } else {
        '\''
    };

    let mut out = String::new();
    out.push(quote);
    for &b in bytes {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'\t' => out.push_str("\\t"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b if b == quote as u8 => {
                out.push('\\');
                out.push(quote);
            }
            0x20..=0x7e => out.push(b as char),
            _ => out.push_str(&format!("\\x{b:02x}")),
        }
    }
    out.push(quote);
    out
}

// ---------------------------------------------------------------------------
// printenv.py
// ---------------------------------------------------------------------------

/// Oils test helper: `printenv.py` prints specified environment variables.
pub struct PrintenvPyCommand;

static PRINTENV_PY_META: CommandMeta = CommandMeta {
    name: "printenv.py",
    synopsis: "printenv.py [NAME ...]",
    description: "Print environment variables or 'None' (Oils test helper).",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for PrintenvPyCommand {
    fn name(&self) -> &str {
        "printenv.py"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&PRINTENV_PY_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut out = String::new();
        for name in args {
            let val = if let Some(vars) = ctx.variables {
                vars.get(name.as_str())
                    .filter(|v| v.exported())
                    .map(|v| v.value.as_scalar().to_string())
            } else {
                ctx.env.get(name.as_str()).cloned()
            };
            match val {
                Some(v) if v.is_empty() => {}
                Some(v) => out.push_str(&format!("{v}\n")),
                None => out.push_str("None\n"),
            }
        }
        CommandResult {
            stdout: out,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// foo=bar
// ---------------------------------------------------------------------------

/// Oils helper command for testing escaped `=` in command names.
pub struct FooEqualsBarCommand;

static FOO_EQUALS_BAR_META: CommandMeta = CommandMeta {
    name: "foo=bar",
    synopsis: "foo=bar",
    description: "Print HI (Oils test helper).",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for FooEqualsBarCommand {
    fn name(&self) -> &str {
        "foo=bar"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&FOO_EQUALS_BAR_META)
    }

    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: "HI\n".to_string(),
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// stdout_stderr.py
// ---------------------------------------------------------------------------

/// Oils test helper: `stdout_stderr.py` prints to stdout and stderr.
///
/// Usage: `stdout_stderr.py [STDOUT [STDERR [STATUS]]]`
pub struct StdoutStderrPyCommand;

static STDOUT_STDERR_PY_META: CommandMeta = CommandMeta {
    name: "stdout_stderr.py",
    synopsis: "stdout_stderr.py [STDOUT [STDERR [STATUS]]]",
    description: "Print to stdout and stderr (Oils test helper).",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for StdoutStderrPyCommand {
    fn name(&self) -> &str {
        "stdout_stderr.py"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&STDOUT_STDERR_PY_META)
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let stdout_val = args.first().map_or("STDOUT", |s| s.as_str());
        let stderr_val = args.get(1).map_or("STDERR", |s| s.as_str());
        let status: i32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
        CommandResult {
            stdout: format!("{stdout_val}\n"),
            stderr: format!("{stderr_val}\n"),
            exit_code: status,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// read_from_fd.py
// ---------------------------------------------------------------------------

/// Oils test helper: print the contents read from stdin or synthetic extra FDs.
pub struct ReadFromFdPyCommand;

static READ_FROM_FD_META: CommandMeta = CommandMeta {
    name: "read_from_fd.py",
    synopsis: "read_from_fd.py FD [FD ...]",
    description: "Print input captured for the requested file descriptors.",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for ReadFromFdPyCommand {
    fn name(&self) -> &str {
        "read_from_fd.py"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&READ_FROM_FD_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut stdout = String::new();
        for arg in args {
            let Ok(fd) = arg.parse::<i32>() else {
                return CommandResult {
                    stderr: format!("read_from_fd.py: invalid fd {arg}\n"),
                    exit_code: 2,
                    ..Default::default()
                };
            };
            let content = if fd == 0 {
                ctx.stdin.to_string()
            } else {
                ctx.env
                    .get(&format!("__RUST_BASH_FD_{fd}"))
                    .cloned()
                    .unwrap_or_default()
            };
            stdout.push_str(&format!("{fd}: {}\n", content.trim_end_matches('\n')));
        }
        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// python2 / python3 (minimal print-only interpreter)
// ---------------------------------------------------------------------------

/// Minimal `python2 -c 'expr'` / `python3 -c 'expr'` helper for Oils tests.
/// Supports only `print("...")` / `print '...'` statements.
pub struct PythonCommand {
    alias: &'static str,
}

impl PythonCommand {
    pub fn python2() -> Self {
        Self { alias: "python2" }
    }
    pub fn python3() -> Self {
        Self { alias: "python3" }
    }
}

static PYTHON_META: CommandMeta = CommandMeta {
    name: "python2",
    synopsis: "python2 -c CODE",
    description: "Minimal Python interpreter for Oils test helpers.",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for PythonCommand {
    fn name(&self) -> &str {
        self.alias
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&PYTHON_META)
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        if args.len() < 2 || args[0] != "-c" {
            return CommandResult {
                stderr: format!("{}: only -c flag is supported\n", self.alias),
                exit_code: 2,
                ..Default::default()
            };
        }
        let code = &args[1];
        let mut stdout = String::new();
        let mut vars = std::collections::HashMap::<String, String>::new();
        for line in code.lines() {
            let trimmed = line.trim();
            if let Some((name, value)) = parse_python_assignment(trimmed) {
                vars.insert(name.to_string(), value);
                continue;
            }
            // Match print("...") or print '...' or print("..." % (...))
            // NOTE: Only supports simple single-argument print(); nested parens
            // like print("a" + str(1)) would mismatch the outer `)`.
            if let Some(inner) = trimmed
                .strip_prefix("print(")
                .and_then(|s| s.strip_suffix(')'))
            {
                let processed = eval_python_print_expr(inner.trim(), &vars);
                stdout.push_str(&processed);
                stdout.push('\n');
            } else if let Some(rest) = trimmed.strip_prefix("print ") {
                // Python 2 style: print 'string'
                let processed = eval_python_print_expr(rest.trim(), &vars);
                stdout.push_str(&processed);
                stdout.push('\n');
            }
        }
        CommandResult {
            stdout,
            exit_code: 0,
            ..Default::default()
        }
    }
}

fn process_python_escapes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => result.push('\n'),
                Some('t') => result.push('\t'),
                Some('\\') => result.push('\\'),
                Some('\'') => result.push('\''),
                Some('"') => result.push('"'),
                Some('u') => {
                    let mut hex = String::new();
                    for _ in 0..4 {
                        match chars.next() {
                            Some(ch) if ch.is_ascii_hexdigit() => hex.push(ch),
                            Some(ch) => {
                                result.push('\\');
                                result.push('u');
                                result.push_str(&hex);
                                result.push(ch);
                                hex.clear();
                                break;
                            }
                            None => break,
                        }
                    }
                    if hex.len() == 4
                        && let Ok(code) = u32::from_str_radix(&hex, 16)
                        && let Some(ch) = char::from_u32(code)
                    {
                        result.push(ch);
                    } else {
                        result.push('\\');
                        result.push('u');
                        result.push_str(&hex);
                    }
                }
                Some('U') => {
                    let mut hex = String::new();
                    for _ in 0..8 {
                        match chars.next() {
                            Some(ch) if ch.is_ascii_hexdigit() => hex.push(ch),
                            Some(ch) => {
                                result.push('\\');
                                result.push('U');
                                result.push_str(&hex);
                                result.push(ch);
                                hex.clear();
                                break;
                            }
                            None => break,
                        }
                    }
                    if hex.len() == 8
                        && let Ok(code) = u32::from_str_radix(&hex, 16)
                        && let Some(ch) = char::from_u32(code)
                    {
                        result.push(ch);
                    } else {
                        result.push('\\');
                        result.push('U');
                        result.push_str(&hex);
                    }
                }
                Some(other) => {
                    result.push('\\');
                    result.push(other);
                }
                None => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    result
}

fn parse_python_assignment(line: &str) -> Option<(&str, String)> {
    let (name, value) = line.split_once('=')?;
    let name = name.trim();
    let value = value.trim();
    let value = value.strip_prefix('u').unwrap_or(value);
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    if (value.starts_with('"') && value.ends_with('"'))
        || (value.starts_with('\'') && value.ends_with('\''))
    {
        Some((name, process_python_escapes(&value[1..value.len() - 1])))
    } else {
        None
    }
}

fn eval_python_print_expr(expr: &str, vars: &std::collections::HashMap<String, String>) -> String {
    if let Some(var_name) = expr
        .strip_suffix(".upper().encode(\"utf-8\")")
        .or_else(|| expr.strip_suffix(".upper().encode('utf-8')"))
    {
        return vars
            .get(var_name.trim())
            .map(|s| {
                s.chars()
                    .map(|c| {
                        let mapped: Vec<char> = c.to_uppercase().collect();
                        if mapped.len() == 1 { mapped[0] } else { c }
                    })
                    .collect()
            })
            .unwrap_or_else(|| expr.to_string());
    }
    if let Some(var_name) = expr
        .strip_suffix(".lower().encode(\"utf-8\")")
        .or_else(|| expr.strip_suffix(".lower().encode('utf-8')"))
    {
        return vars
            .get(var_name.trim())
            .map(|s| {
                s.chars()
                    .map(|c| {
                        let mapped: Vec<char> = c.to_lowercase().collect();
                        if mapped.len() == 1 { mapped[0] } else { c }
                    })
                    .collect()
            })
            .unwrap_or_else(|| expr.to_string());
    }
    if let Some(value) = vars.get(expr) {
        return value.clone();
    }
    let s = if (expr.starts_with('"') && expr.ends_with('"'))
        || (expr.starts_with('\'') && expr.ends_with('\''))
    {
        &expr[1..expr.len() - 1]
    } else {
        expr
    };
    process_python_escapes(s)
}

/// Register all Oils test helper commands on a builder.
pub fn register_oils_helpers(
    mut builder: rust_bash::RustBashBuilder,
) -> rust_bash::RustBashBuilder {
    use std::sync::Arc;
    builder = builder
        .command(Arc::new(ArgvPyCommand))
        .command(Arc::new(FooEqualsBarCommand))
        .command(Arc::new(PrintenvPyCommand))
        .command(Arc::new(ReadFromFdPyCommand))
        .command(Arc::new(StdoutStderrPyCommand))
        .command(Arc::new(PythonCommand::python2()))
        .command(Arc::new(PythonCommand::python3()));
    builder
}
