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
    let mut base = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => base.push_str("\\\\"),
            '\t' => base.push_str("\\t"),
            '\n' => base.push_str("\\n"),
            '\r' => base.push_str("\\r"),
            _ if c.is_ascii_control() => {
                base.push_str(&format!("\\x{:02x}", c as u32));
            }
            _ => base.push(c),
        }
    }
    if s.contains('\'') && !s.contains('"') {
        let escaped = base.replace('"', "\\\"");
        format!("\"{escaped}\"")
    } else {
        let escaped = base.replace('\'', "\\'");
        format!("'{escaped}'")
    }
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
        for line in code.lines() {
            let trimmed = line.trim();
            // Match print("...") or print '...' or print("..." % (...))
            // NOTE: Only supports simple single-argument print(); nested parens
            // like print("a" + str(1)) would mismatch the outer `)`.
            if let Some(inner) = trimmed
                .strip_prefix("print(")
                .and_then(|s| s.strip_suffix(')'))
            {
                let s = if (inner.starts_with('"') && inner.ends_with('"'))
                    || (inner.starts_with('\'') && inner.ends_with('\''))
                {
                    &inner[1..inner.len() - 1]
                } else {
                    inner
                };
                let processed = process_python_escapes(s);
                stdout.push_str(&processed);
                stdout.push('\n');
            } else if let Some(rest) = trimmed.strip_prefix("print ") {
                // Python 2 style: print 'string'
                let s = rest.trim();
                let s = if (s.starts_with('"') && s.ends_with('"'))
                    || (s.starts_with('\'') && s.ends_with('\''))
                {
                    &s[1..s.len() - 1]
                } else {
                    s
                };
                let processed = process_python_escapes(s);
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

/// Register all Oils test helper commands on a builder.
pub fn register_oils_helpers(
    mut builder: rust_bash::RustBashBuilder,
) -> rust_bash::RustBashBuilder {
    use std::sync::Arc;
    builder = builder
        .command(Arc::new(ArgvPyCommand))
        .command(Arc::new(FooEqualsBarCommand))
        .command(Arc::new(PrintenvPyCommand))
        .command(Arc::new(StdoutStderrPyCommand))
        .command(Arc::new(PythonCommand::python2()))
        .command(Arc::new(PythonCommand::python3()));
    builder
}
