//! Interpreter engine: parsing, AST walking, and execution state.

pub(crate) mod arithmetic;
pub(crate) mod brace;
mod builtins;
mod expansion;
pub(crate) mod pattern;
mod walker;

use crate::commands::VirtualCommand;
use crate::error::RustBashError;
use crate::vfs::VirtualFs;
use brush_parser::ast;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use expansion::expand_word;
pub use walker::execute_program;

// ── Core types ───────────────────────────────────────────────────────

/// Signal for loop control flow (`break`, `continue`) and function return.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlFlow {
    Break(usize),
    Continue(usize),
    Return(i32),
}

/// Result of executing a shell command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// A shell variable with metadata.
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: String,
    pub exported: bool,
    pub readonly: bool,
}

/// Execution limits (no enforcement in Phase 1A).
#[derive(Debug, Clone)]
pub struct ExecutionLimits {
    pub max_call_depth: usize,
    pub max_command_count: usize,
    pub max_loop_iterations: usize,
    pub max_execution_time: Duration,
    pub max_output_size: usize,
    pub max_string_length: usize,
    pub max_glob_results: usize,
    pub max_substitution_depth: usize,
    pub max_heredoc_size: usize,
    pub max_brace_expansion: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_call_depth: 100,
            max_command_count: 10_000,
            max_loop_iterations: 10_000,
            max_execution_time: Duration::from_secs(30),
            max_output_size: 10 * 1024 * 1024,
            max_string_length: 10 * 1024 * 1024,
            max_glob_results: 100_000,
            max_substitution_depth: 50,
            max_heredoc_size: 10 * 1024 * 1024,
            max_brace_expansion: 10_000,
        }
    }
}

/// Execution counters, reset per `exec()` call.
#[derive(Debug, Clone)]
pub struct ExecutionCounters {
    pub command_count: usize,
    pub call_depth: usize,
    pub output_size: usize,
    pub start_time: Instant,
}

impl Default for ExecutionCounters {
    fn default() -> Self {
        Self {
            command_count: 0,
            call_depth: 0,
            output_size: 0,
            start_time: Instant::now(),
        }
    }
}

impl ExecutionCounters {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Shell options (flags only, no enforcement in Phase 1A).
#[derive(Debug, Clone, Default)]
pub struct ShellOpts {
    pub errexit: bool,
    pub nounset: bool,
    pub pipefail: bool,
    pub xtrace: bool,
}

/// Stub for function definitions (execution in a future phase).
#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub body: ast::FunctionBody,
}

/// The interpreter's mutable state, persistent across `exec()` calls.
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
    pub(crate) should_exit: bool,
    pub(crate) loop_depth: usize,
    pub(crate) control_flow: Option<ControlFlow>,
    pub positional_params: Vec<String>,
    pub shell_name: String,
    /// Simple PRNG state for $RANDOM.
    pub(crate) random_seed: u32,
    /// Stack of restore maps for `local` variable scoping in functions.
    pub(crate) local_scopes: Vec<HashMap<String, Option<Variable>>>,
    /// How many function calls deep we are (for `local`/`return` validation).
    pub(crate) in_function_depth: usize,
    /// Registered trap handlers: signal/event name → command string.
    pub(crate) traps: HashMap<String, String>,
    /// True while executing a trap handler (prevents recursive re-trigger).
    pub(crate) in_trap: bool,
    /// Nesting depth for contexts where `set -e` should NOT trigger an exit.
    /// Incremented when entering if/while/until conditions, `&&`/`||` left sides, or `!` pipelines.
    pub(crate) errexit_suppressed: usize,
}

// ── Parsing ──────────────────────────────────────────────────────────

pub(crate) fn parser_options() -> brush_parser::ParserOptions {
    brush_parser::ParserOptions {
        sh_mode: false,
        posix_mode: false,
        enable_extended_globbing: true,
        tilde_expansion: true,
    }
}

/// Parse a shell input string into an AST.
pub fn parse(input: &str) -> Result<ast::Program, RustBashError> {
    let tokens =
        brush_parser::tokenize_str(input).map_err(|e| RustBashError::Parse(e.to_string()))?;

    if tokens.is_empty() {
        return Ok(ast::Program {
            complete_commands: vec![],
        });
    }

    let options = parser_options();
    let source_info = brush_parser::SourceInfo {
        source: input.to_string(),
    };

    brush_parser::parse_tokens(&tokens, &options, &source_info)
        .map_err(|e| RustBashError::Parse(e.to_string()))
}

/// Set a variable in the interpreter state, respecting readonly.
pub(crate) fn set_variable(
    state: &mut InterpreterState,
    name: &str,
    value: String,
) -> Result<(), RustBashError> {
    if let Some(var) = state.env.get(name)
        && var.readonly
    {
        return Err(RustBashError::Execution(format!(
            "{name}: readonly variable"
        )));
    }
    match state.env.get_mut(name) {
        Some(var) => var.value = value,
        None => {
            state.env.insert(
                name.to_string(),
                Variable {
                    value,
                    exported: false,
                    readonly: false,
                },
            );
        }
    }
    Ok(())
}

/// Generate next pseudo-random number (xorshift32, range 0..32767).
pub(crate) fn next_random(state: &mut InterpreterState) -> u16 {
    let mut s = state.random_seed;
    if s == 0 {
        s = 12345;
    }
    s ^= s << 13;
    s ^= s >> 17;
    s ^= s << 5;
    state.random_seed = s;
    (s & 0x7FFF) as u16
}

/// Execute a trap handler string, preventing recursive re-trigger of the same trap type.
pub(crate) fn execute_trap(
    trap_cmd: &str,
    state: &mut InterpreterState,
) -> Result<ExecResult, RustBashError> {
    let was_in_trap = state.in_trap;
    state.in_trap = true;
    let program = parse(trap_cmd)?;
    let result = walker::execute_program(&program, state);
    state.in_trap = was_in_trap;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_input() {
        let program = parse("").unwrap();
        assert!(program.complete_commands.is_empty());
    }

    #[test]
    fn parse_simple_command() {
        let program = parse("echo hello").unwrap();
        assert_eq!(program.complete_commands.len(), 1);
    }

    #[test]
    fn parse_sequential_commands() {
        let program = parse("echo a; echo b").unwrap();
        assert!(!program.complete_commands.is_empty());
    }

    #[test]
    fn parse_pipeline() {
        let program = parse("echo hello | cat").unwrap();
        assert_eq!(program.complete_commands.len(), 1);
    }

    #[test]
    fn parse_and_or() {
        let program = parse("true && echo yes").unwrap();
        assert_eq!(program.complete_commands.len(), 1);
    }

    #[test]
    fn parse_error_on_unclosed_quote() {
        let result = parse("echo 'unterminated");
        assert!(result.is_err());
    }

    #[test]
    fn expand_simple_text() {
        let word = ast::Word {
            value: "hello".to_string(),
            loc: None,
        };
        let state = make_test_state();
        assert_eq!(expand_word(&word, &state).unwrap(), vec!["hello"]);
    }

    #[test]
    fn expand_single_quoted_text() {
        let word = ast::Word {
            value: "'hello world'".to_string(),
            loc: None,
        };
        let state = make_test_state();
        assert_eq!(expand_word(&word, &state).unwrap(), vec!["hello world"]);
    }

    #[test]
    fn expand_double_quoted_text() {
        let word = ast::Word {
            value: "\"hello world\"".to_string(),
            loc: None,
        };
        let state = make_test_state();
        assert_eq!(expand_word(&word, &state).unwrap(), vec!["hello world"]);
    }

    #[test]
    fn expand_escaped_character() {
        let word = ast::Word {
            value: "hello\\ world".to_string(),
            loc: None,
        };
        let state = make_test_state();
        assert_eq!(expand_word(&word, &state).unwrap(), vec!["hello world"]);
    }

    fn make_test_state() -> InterpreterState {
        use crate::vfs::InMemoryFs;
        InterpreterState {
            fs: Arc::new(InMemoryFs::new()),
            env: HashMap::new(),
            cwd: "/".to_string(),
            functions: HashMap::new(),
            last_exit_code: 0,
            commands: HashMap::new(),
            shell_opts: ShellOpts::default(),
            limits: ExecutionLimits::default(),
            counters: ExecutionCounters::default(),
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
        }
    }
}
