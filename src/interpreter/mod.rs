//! Interpreter engine: parsing, AST walking, and execution state.

pub(crate) mod arithmetic;
pub(crate) mod brace;
pub(crate) mod builtins;
mod expansion;
pub(crate) mod pattern;
mod walker;

use crate::commands::VirtualCommand;
use crate::error::RustBashError;
use crate::network::NetworkPolicy;
use crate::platform::Instant;
use crate::vfs::VirtualFs;
use bitflags::bitflags;
use brush_parser::ast;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

pub use builtins::builtin_names;
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
    /// Binary output for commands that produce non-text data.
    pub stdout_bytes: Option<Vec<u8>>,
}

// ── Variable types ──────────────────────────────────────────────────

/// The value stored in a shell variable: scalar, indexed array, or associative array.
#[derive(Debug, Clone, PartialEq)]
pub enum VariableValue {
    Scalar(String),
    IndexedArray(BTreeMap<usize, String>),
    AssociativeArray(BTreeMap<String, String>),
}

impl VariableValue {
    /// Return the scalar value, or element \[0\] for indexed arrays,
    /// or empty string for associative arrays (matches bash behavior).
    pub fn as_scalar(&self) -> &str {
        match self {
            VariableValue::Scalar(s) => s,
            VariableValue::IndexedArray(map) => map.get(&0).map(|s| s.as_str()).unwrap_or(""),
            VariableValue::AssociativeArray(map) => map.get("0").map(|s| s.as_str()).unwrap_or(""),
        }
    }

    /// Return element count for arrays, or 1 for non-empty scalars.
    pub fn count(&self) -> usize {
        match self {
            VariableValue::Scalar(s) => usize::from(!s.is_empty()),
            VariableValue::IndexedArray(map) => map.len(),
            VariableValue::AssociativeArray(map) => map.len(),
        }
    }
}

bitflags! {
    /// Attribute flags for a shell variable.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct VariableAttrs: u8 {
        const EXPORTED  = 0b0000_0001;
        const READONLY  = 0b0000_0010;
        const INTEGER   = 0b0000_0100;
        const LOWERCASE = 0b0000_1000;
        const UPPERCASE = 0b0001_0000;
        const NAMEREF   = 0b0010_0000;
        /// Variable was declared but never assigned a value.
        const DECLARED_ONLY = 0b0100_0000;
        /// `declare -p` should emit associative entries in bash's reverse hash-style order.
        const ASSOC_REVERSE_PRINT = 0b1000_0000;
    }
}

/// A shell variable with metadata.
#[derive(Debug, Clone)]
pub struct Variable {
    pub value: VariableValue,
    pub attrs: VariableAttrs,
}

/// A persistent file descriptor redirection established by `exec`.
#[derive(Debug, Clone)]
pub(crate) enum PersistentFd {
    /// FD writes to this VFS path.
    OutputFile(String),
    /// FD reads from this VFS path.
    InputFile(String),
    /// FD is open for both reading and writing on this VFS path.
    ReadWriteFile(String),
    /// FD points to /dev/null (reads empty, writes discarded).
    DevNull,
    /// FD is closed.
    Closed,
    /// FD is a duplicate of a standard fd (0=stdin, 1=stdout, 2=stderr).
    DupStdFd(i32),
}

impl Variable {
    /// Convenience: is this variable exported?
    pub fn exported(&self) -> bool {
        self.attrs.contains(VariableAttrs::EXPORTED)
    }

    /// Convenience: is this variable readonly?
    pub fn readonly(&self) -> bool {
        self.attrs.contains(VariableAttrs::READONLY)
    }
}

/// Execution limits.
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
    pub max_array_elements: usize,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_call_depth: 50,
            max_command_count: 10_000,
            max_loop_iterations: 10_000,
            max_execution_time: Duration::from_secs(30),
            max_output_size: 10 * 1024 * 1024,
            max_string_length: 10 * 1024 * 1024,
            max_glob_results: 100_000,
            max_substitution_depth: 50,
            max_heredoc_size: 10 * 1024 * 1024,
            max_brace_expansion: 10_000,
            max_array_elements: 100_000,
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
    pub substitution_depth: usize,
}

impl Default for ExecutionCounters {
    fn default() -> Self {
        Self {
            command_count: 0,
            call_depth: 0,
            output_size: 0,
            start_time: Instant::now(),
            substitution_depth: 0,
        }
    }
}

impl ExecutionCounters {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Shell options controlled by `set -o` / `set +o` and single-letter flags.
#[derive(Debug, Clone, Default)]
pub struct ShellOpts {
    pub errexit: bool,
    pub nounset: bool,
    pub pipefail: bool,
    pub xtrace: bool,
    pub verbose: bool,
    pub noexec: bool,
    pub noclobber: bool,
    pub allexport: bool,
    pub noglob: bool,
    pub posix: bool,
    pub vi_mode: bool,
    pub emacs_mode: bool,
}

/// Shopt options (`shopt -s`/`-u` flags).
#[derive(Debug, Clone)]
pub struct ShoptOpts {
    pub strict_arg_parse: bool,
    pub strict_argv: bool,
    pub strict_array: bool,
    pub nullglob: bool,
    pub globstar: bool,
    pub dotglob: bool,
    pub globskipdots: bool,
    pub failglob: bool,
    pub nocaseglob: bool,
    pub nocasematch: bool,
    pub lastpipe: bool,
    pub expand_aliases: bool,
    pub xpg_echo: bool,
    pub extglob: bool,
    pub progcomp: bool,
    pub hostcomplete: bool,
    pub complete_fullquote: bool,
    pub sourcepath: bool,
    pub promptvars: bool,
    pub interactive_comments: bool,
    pub cmdhist: bool,
    pub lithist: bool,
    pub autocd: bool,
    pub cdspell: bool,
    pub dirspell: bool,
    pub direxpand: bool,
    pub checkhash: bool,
    pub checkjobs: bool,
    pub checkwinsize: bool,
    pub extquote: bool,
    pub force_fignore: bool,
    pub globasciiranges: bool,
    pub gnu_errfmt: bool,
    pub histappend: bool,
    pub histreedit: bool,
    pub histverify: bool,
    pub huponexit: bool,
    pub inherit_errexit: bool,
    pub login_shell: bool,
    pub mailwarn: bool,
    pub no_empty_cmd_completion: bool,
    pub progcomp_alias: bool,
    pub shift_verbose: bool,
    pub execfail: bool,
    pub cdable_vars: bool,
    pub localvar_inherit: bool,
    pub localvar_unset: bool,
    pub extdebug: bool,
    pub strict_arith: bool,
    pub patsub_replacement: bool,
    pub assoc_expand_once: bool,
    pub varredir_close: bool,
}

impl Default for ShoptOpts {
    fn default() -> Self {
        Self {
            strict_arg_parse: false,
            strict_argv: false,
            strict_array: false,
            nullglob: false,
            globstar: false,
            dotglob: false,
            globskipdots: true,
            failglob: false,
            nocaseglob: false,
            nocasematch: false,
            lastpipe: false,
            expand_aliases: false,
            xpg_echo: false,
            extglob: true,
            progcomp: true,
            hostcomplete: true,
            complete_fullquote: true,
            sourcepath: true,
            promptvars: true,
            interactive_comments: true,
            cmdhist: true,
            lithist: false,
            autocd: false,
            cdspell: false,
            dirspell: false,
            direxpand: false,
            checkhash: false,
            checkjobs: false,
            checkwinsize: true,
            extquote: true,
            force_fignore: true,
            globasciiranges: true,
            gnu_errfmt: false,
            histappend: false,
            histreedit: false,
            histverify: false,
            huponexit: false,
            inherit_errexit: false,
            login_shell: false,
            mailwarn: false,
            no_empty_cmd_completion: false,
            progcomp_alias: false,
            shift_verbose: false,
            execfail: false,
            cdable_vars: false,
            localvar_inherit: false,
            localvar_unset: false,
            extdebug: false,
            strict_arith: false,
            patsub_replacement: true,
            assoc_expand_once: false,
            varredir_close: false,
        }
    }
}

/// Stub for function definitions (execution in a future phase).
#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub body: ast::FunctionBody,
    pub source: String,
    pub source_text: String,
    pub lineno: usize,
}

/// A single frame on the function call stack, used to expose
/// `FUNCNAME`, `BASH_SOURCE`, and `BASH_LINENO` arrays.
#[derive(Debug, Clone)]
pub struct CallFrame {
    pub func_name: String,
    pub source: String,
    pub lineno: usize,
}

/// The interpreter's mutable state, persistent across `exec()` calls.
pub struct InterpreterState {
    pub fs: Arc<dyn VirtualFs>,
    pub env: HashMap<String, Variable>,
    pub cwd: String,
    pub functions: HashMap<String, FunctionDef>,
    pub last_exit_code: i32,
    pub commands: HashMap<String, Arc<dyn VirtualCommand>>,
    pub shell_opts: ShellOpts,
    pub shopt_opts: ShoptOpts,
    pub limits: ExecutionLimits,
    pub counters: ExecutionCounters,
    pub network_policy: NetworkPolicy,
    pub(crate) should_exit: bool,
    pub(crate) loop_depth: usize,
    pub(crate) control_flow: Option<ControlFlow>,
    pub positional_params: Vec<String>,
    pub shell_name: String,
    /// Simple PRNG state for $RANDOM.
    pub(crate) random_seed: u32,
    /// Stack of restore maps for `local` variable scoping in functions.
    pub(crate) local_scopes: Vec<HashMap<String, Option<Variable>>>,
    /// Stack of temporary prefix-assignment frames active for the current command.
    pub(crate) temp_binding_scopes: Vec<HashMap<String, Option<Variable>>>,
    /// How many function calls deep we are (for `local`/`return` validation).
    pub(crate) in_function_depth: usize,
    /// Registered trap handlers: signal/event name → command string.
    pub(crate) traps: HashMap<String, String>,
    /// True while executing a trap handler (prevents recursive re-trigger).
    pub(crate) in_trap: bool,
    /// Nesting depth for contexts where `set -e` should NOT trigger an exit.
    /// Incremented when entering if/while/until conditions, `&&`/`||` left sides, or `!` pipelines.
    pub(crate) errexit_suppressed: usize,
    /// Byte offset into the current stdin stream, used by `read` to consume
    /// successive lines from piped input across loop iterations.
    pub(crate) stdin_offset: usize,
    /// Directory stack for `pushd`/`popd`/`dirs`.
    pub(crate) dir_stack: Vec<String>,
    /// Cached command-name → resolved-path mappings for `hash`.
    pub(crate) command_hash: HashMap<String, String>,
    /// Alias name → expansion string for `alias`/`unalias`.
    pub(crate) aliases: HashMap<String, String>,
    /// Current line number, updated per-statement from AST source positions.
    pub(crate) current_lineno: usize,
    /// Current source file or synthetic top-level label for function metadata.
    pub(crate) current_source: String,
    /// Full source text for the currently executing parsed program.
    pub(crate) current_source_text: String,
    /// Shell start time for `$SECONDS`.
    pub(crate) shell_start_time: Instant,
    /// Last argument of the previous simple command (`$_`).
    pub(crate) last_argument: String,
    /// Function call stack for `FUNCNAME`, `BASH_SOURCE`, `BASH_LINENO`.
    pub(crate) call_stack: Vec<CallFrame>,
    /// Configurable `$MACHTYPE` value.
    pub(crate) machtype: String,
    /// Configurable `$HOSTTYPE` value.
    pub(crate) hosttype: String,
    /// Persistent FD redirections set by `exec` (e.g. `exec > file`).
    pub(crate) persistent_fds: HashMap<i32, PersistentFd>,
    /// Next auto-allocated FD number for `{varname}>file` syntax.
    pub(crate) next_auto_fd: i32,
    /// Counter for generating unique process substitution temp file names.
    pub(crate) proc_sub_counter: u64,
    /// Pre-allocated temp file paths for redirect process substitutions, keyed by
    /// the pointer address of the `IoFileRedirectTarget` AST node.  This ensures
    /// each redirect resolves to its own pre-allocated path regardless of the order
    /// in which input/output redirect resolution visits them.
    pub(crate) proc_sub_prealloc: HashMap<usize, String>,
    /// Binary data from the previous pipeline stage, set by `execute_pipeline()`
    /// and consumed by `dispatch_command()` to populate `CommandContext::stdin_bytes`.
    pub(crate) pipe_stdin_bytes: Option<Vec<u8>>,
    /// Stderr accumulated from command substitutions during word expansion.
    /// Drained by the enclosing command execution into its `ExecResult.stderr`.
    pub(crate) pending_cmdsub_stderr: String,
    /// Set when a fatal parameter expansion error terminates the current shell.
    pub(crate) fatal_expansion_error: bool,
    /// Distinguishes shell/runtime errors from ordinary non-zero command exits.
    pub(crate) last_command_had_error: bool,
}

// ── Parsing ──────────────────────────────────────────────────────────

pub(crate) fn parser_options() -> brush_parser::ParserOptions {
    brush_parser::ParserOptions {
        sh_mode: false,
        posix_mode: false,
        enable_extended_globbing: true,
        tilde_expansion_at_word_start: true,
        tilde_expansion_after_colon: true,
        ..Default::default()
    }
}

/// Parse a shell input string into an AST.
pub fn parse(input: &str) -> Result<ast::Program, RustBashError> {
    let mut parse_input =
        rewrite_legacy_ksh_command_substitutions(input).unwrap_or_else(|| input.to_string());
    let raw_tokens = match brush_parser::tokenize_str(&parse_input) {
        Ok(tokens) => tokens,
        Err(err) => {
            if let Some(rewritten) = rewrite_expansion_like_heredoc_delimiters(input)
                && let Ok(tokens) = brush_parser::tokenize_str(&rewritten)
            {
                parse_input = rewritten;
                tokens
            } else {
                return Err(RustBashError::Parse(err.to_string()));
            }
        }
    };
    let tokens = rebuild_tokens_from_source(&parse_input, &raw_tokens);

    if tokens.is_empty() {
        return Ok(ast::Program {
            complete_commands: vec![],
        });
    }

    let options = parser_options();

    match brush_parser::parse_tokens(&tokens, &options) {
        Ok(program) => Ok(program),
        Err(err) => {
            if let Some(rewritten) = rewrite_assignment_prefixed_keyword(&parse_input, &tokens) {
                let retry_raw_tokens = brush_parser::tokenize_str(&rewritten)
                    .map_err(|e| RustBashError::Parse(e.to_string()))?;
                let retry_tokens = rebuild_tokens_from_source(&rewritten, &retry_raw_tokens);
                if let Ok(program) = brush_parser::parse_tokens(&retry_tokens, &options) {
                    return Ok(program);
                }
            }
            if let Some(rewritten) = rewrite_extended_test_unary_literal_operands(&parse_input) {
                let retry_raw_tokens = brush_parser::tokenize_str(&rewritten)
                    .map_err(|e| RustBashError::Parse(e.to_string()))?;
                let retry_tokens = rebuild_tokens_from_source(&rewritten, &retry_raw_tokens);
                if let Ok(program) = brush_parser::parse_tokens(&retry_tokens, &options) {
                    return Ok(program);
                }
            }
            Err(RustBashError::Parse(err.to_string()))
        }
    }
}

fn rewrite_legacy_ksh_command_substitutions(input: &str) -> Option<String> {
    if !input.contains("${") {
        return None;
    }

    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut changed = false;
    let mut quote: Option<char> = None;

    while i < chars.len() {
        let ch = chars[i];
        if let Some(active_quote) = quote {
            out.push(ch);
            if ch == active_quote {
                quote = None;
            } else if ch == '\\' && active_quote == '"' && i + 1 < chars.len() {
                i += 1;
                out.push(chars[i]);
            }
            i += 1;
            continue;
        }

        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            out.push(ch);
            i += 1;
            continue;
        }

        if ch == '$'
            && i + 1 < chars.len()
            && chars[i + 1] == '{'
            && let Some((token, next_idx)) = take_heredoc_delimiter_token(&chars, i)
            && let Some(rewritten) = rewrite_single_legacy_ksh_token(&token)
        {
            out.push_str(&rewritten);
            changed = true;
            i = next_idx;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    changed.then_some(out)
}

fn rewrite_single_legacy_ksh_token(token: &str) -> Option<String> {
    if !(token.starts_with("${") && token.ends_with('}')) {
        return None;
    }

    let inner = &token[2..token.len() - 1];
    if let Some(rest) = inner.strip_prefix('|') {
        if rest.chars().next().is_none_or(|ch| ch.is_whitespace()) {
            return None;
        }
        let body = normalize_legacy_ksh_command_body(trim_legacy_ksh_command_body(rest)?);
        return Some(format!(
            "$( {{ BRUSH_LEGACY_KSH_REPLY=1; {body}; printf '%s' \"$REPLY\"; }} )"
        ));
    }

    if !inner.chars().next().is_some_and(|ch| ch.is_whitespace()) {
        return None;
    }

    let trimmed = inner.trim_start();
    if trimmed.starts_with('|') {
        return None;
    }

    let body = normalize_legacy_ksh_command_body(trim_legacy_ksh_command_body(trimmed)?);
    Some(format!("$({body})"))
}

fn trim_legacy_ksh_command_body(body: &str) -> Option<String> {
    let mut trimmed = body.trim_end();
    if let Some(stripped) = trimmed.strip_suffix(';') {
        trimmed = stripped.trim_end();
    }
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn normalize_legacy_ksh_command_body(body: String) -> String {
    if !body.trim_start().starts_with("case ") {
        return body;
    }

    if let Some(in_pos) = body.find(" in ") {
        let clause_start = in_pos + 4;
        if body
            .get(clause_start..)
            .is_some_and(|rest| !rest.starts_with('('))
        {
            let mut normalized = body;
            normalized.insert(clause_start, '(');
            return normalized;
        }
    }

    body
}

fn rewrite_expansion_like_heredoc_delimiters(input: &str) -> Option<String> {
    if !input.contains("<<$") {
        return None;
    }

    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut changed = false;

    while i < chars.len() {
        if chars[i] == '<' && i + 1 < chars.len() && chars[i + 1] == '<' {
            out.push('<');
            out.push('<');
            i += 2;
            if i < chars.len() && chars[i] == '-' {
                out.push('-');
                i += 1;
            }
            while i < chars.len() && matches!(chars[i], ' ' | '\t') {
                out.push(chars[i]);
                i += 1;
            }
            if i + 1 < chars.len()
                && chars[i] == '$'
                && matches!(chars[i + 1], '{' | '(')
                && let Some((token, next_idx)) = take_heredoc_delimiter_token(&chars, i)
            {
                out.push('\'');
                out.push_str(&token);
                out.push('\'');
                i = next_idx;
                changed = true;
                continue;
            }
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    changed.then_some(out)
}

fn take_heredoc_delimiter_token(chars: &[char], start: usize) -> Option<(String, usize)> {
    if start + 1 >= chars.len() || chars[start] != '$' {
        return None;
    }

    let closing = match chars[start + 1] {
        '{' => '}',
        '(' => ')',
        _ => return None,
    };

    let mut token = String::new();
    let mut depth = 0usize;
    let mut i = start;
    while i < chars.len() {
        let ch = chars[i];
        token.push(ch);
        if ch == chars[start + 1] {
            depth += 1;
        } else if ch == closing {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                return Some((token, i + 1));
            }
        }
        i += 1;
    }
    None
}

fn rebuild_tokens_from_source(
    input: &str,
    tokens: &[brush_parser::Token],
) -> Vec<brush_parser::Token> {
    tokens
        .iter()
        .map(|token| match token {
            brush_parser::Token::Word(text, loc) => {
                let source_text = slice_source_by_char_range(input, loc.start.index, loc.end.index);
                if let Some(source_text) = source_text
                    && source_text != *text
                    && collapse_space_runs(&source_text) == *text
                {
                    brush_parser::Token::Word(source_text, loc.clone())
                } else {
                    token.clone()
                }
            }
            brush_parser::Token::Operator(_, _) => token.clone(),
        })
        .collect()
}

fn collapse_space_runs(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch == ' ' {
            if !prev_space {
                out.push(ch);
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
}

fn slice_source_by_char_range(input: &str, start: usize, end: usize) -> Option<String> {
    if start > end {
        return None;
    }

    let total_chars = input.chars().count();
    if end > total_chars {
        return None;
    }

    let start_byte = if start == total_chars {
        input.len()
    } else {
        input.char_indices().nth(start)?.0
    };
    let end_byte = if end == total_chars {
        input.len()
    } else {
        input.char_indices().nth(end)?.0
    };
    input.get(start_byte..end_byte).map(ToString::to_string)
}

fn rewrite_assignment_prefixed_keyword(
    input: &str,
    tokens: &[brush_parser::Token],
) -> Option<String> {
    const RESERVED_WORDS: &[&str] = &[
        "case", "do", "done", "elif", "else", "esac", "fi", "for", "if", "in", "select", "then",
        "until", "while",
    ];

    if tokens.len() < 2 {
        return None;
    }

    let last = tokens.last()?;
    if !matches!(last, brush_parser::Token::Word(_, _)) {
        return None;
    }

    let last_text = last.to_str();
    if !RESERVED_WORDS.contains(&last_text) {
        return None;
    }

    if !tokens[..tokens.len() - 1]
        .iter()
        .all(is_simple_assignment_word_token)
    {
        return None;
    }

    let start = last.location().start.index;
    let mut rewritten = String::with_capacity(input.len() + 1);
    rewritten.push_str(&input[..start]);
    rewritten.push('\\');
    rewritten.push_str(&input[start..]);
    Some(rewritten)
}

fn rewrite_extended_test_unary_literal_operands(input: &str) -> Option<String> {
    if !input.contains("[[") {
        return None;
    }

    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;
    let mut changed = false;
    let mut quote: Option<char> = None;

    while i < chars.len() {
        let ch = chars[i];
        if let Some(active_quote) = quote {
            out.push(ch);
            if ch == active_quote {
                quote = None;
            } else if ch == '\\' && active_quote == '"' && i + 1 < chars.len() {
                i += 1;
                out.push(chars[i]);
            }
            i += 1;
            continue;
        }

        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            out.push(ch);
            i += 1;
            continue;
        }

        if ch == '['
            && i + 1 < chars.len()
            && chars[i + 1] == '['
            && let Some(end) = find_extended_test_end(&chars, i + 2)
        {
            let segment: String = chars[i..end + 2].iter().collect();
            if let Some(rewritten) = rewrite_single_extended_test_segment(&segment) {
                out.push_str(&rewritten);
                changed = true;
            } else {
                out.push_str(&segment);
            }
            i = end + 2;
            continue;
        }

        out.push(ch);
        i += 1;
    }

    changed.then_some(out)
}

fn find_extended_test_end(chars: &[char], start: usize) -> Option<usize> {
    let mut i = start;
    let mut quote: Option<char> = None;

    while i + 1 < chars.len() {
        let ch = chars[i];
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else if ch == '\\' && active_quote == '"' {
                i += 1;
            }
            i += 1;
            continue;
        }

        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            i += 1;
            continue;
        }

        if ch == ']' && chars[i + 1] == ']' {
            return Some(i);
        }

        i += 1;
    }

    None
}

fn rewrite_single_extended_test_segment(segment: &str) -> Option<String> {
    if !segment.starts_with("[[") || !segment.ends_with("]]") {
        return None;
    }

    let inner = &segment[2..segment.len() - 2];
    let tokens: Vec<&str> = inner.split_whitespace().collect();
    if tokens.len() == 2
        && is_extended_test_unary_predicate(tokens[0])
        && matches!(tokens[1], "=" | "==")
    {
        return Some(format!("[[ {} '{}' ]]", tokens[0], tokens[1]));
    }

    None
}

fn is_extended_test_unary_predicate(token: &str) -> bool {
    matches!(
        token,
        "-a" | "-b"
            | "-c"
            | "-d"
            | "-e"
            | "-f"
            | "-g"
            | "-h"
            | "-k"
            | "-n"
            | "-o"
            | "-p"
            | "-r"
            | "-s"
            | "-t"
            | "-u"
            | "-v"
            | "-w"
            | "-x"
            | "-z"
            | "-G"
            | "-L"
            | "-N"
            | "-O"
            | "-R"
            | "-S"
    )
}

fn is_simple_assignment_word_token(token: &brush_parser::Token) -> bool {
    let brush_parser::Token::Word(text, _) = token else {
        return false;
    };
    let Some((name, _value)) = text.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphabetic() && first != '_' {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Set a variable in the interpreter state, respecting readonly, nameref,
/// and attribute transforms (INTEGER, LOWERCASE, UPPERCASE).
pub(crate) fn set_variable(
    state: &mut InterpreterState,
    name: &str,
    value: String,
) -> Result<(), RustBashError> {
    if value.len() > state.limits.max_string_length {
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_string_length",
            limit_value: state.limits.max_string_length,
            actual_value: value.len(),
        });
    }

    // Resolve nameref chain to find the actual target variable.
    let target = resolve_nameref(name, state)?;

    // If the resolved target is an array subscript (e.g. from a nameref to "a[2]"),
    // set the array element directly.
    if let Some(bracket_pos) = target.find('[')
        && target.ends_with(']')
    {
        let arr_name = &target[..bracket_pos];
        let index_raw = &target[bracket_pos + 1..target.len() - 1];
        // Expand variables and strip quotes from the index.
        let word = brush_parser::ast::Word {
            value: index_raw.to_string(),
            loc: None,
        };
        let expanded_key = crate::interpreter::expansion::expand_word_to_string_mut(&word, state)?;

        if let Some(var) = state.env.get(arr_name)
            && var.readonly()
        {
            return Err(RustBashError::Execution(format!(
                "{arr_name}: readonly variable"
            )));
        }

        // Determine variable type and evaluate index before mutable borrow.
        let is_assoc = state
            .env
            .get(arr_name)
            .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)));
        let numeric_idx = if !is_assoc {
            crate::interpreter::arithmetic::eval_arithmetic(&expanded_key, state).unwrap_or(0)
        } else {
            0
        };

        match state.env.get_mut(arr_name) {
            Some(var) => match &mut var.value {
                VariableValue::AssociativeArray(map) => {
                    map.insert(expanded_key, value);
                }
                VariableValue::IndexedArray(map) => {
                    let actual_idx = if numeric_idx < 0 {
                        let max_key = map.keys().next_back().copied().unwrap_or(0);
                        let resolved = max_key as i64 + 1 + numeric_idx;
                        if resolved < 0 {
                            0usize
                        } else {
                            resolved as usize
                        }
                    } else {
                        numeric_idx as usize
                    };
                    map.insert(actual_idx, value);
                }
                VariableValue::Scalar(s) => {
                    if numeric_idx == 0 || numeric_idx == -1 {
                        *s = value;
                    }
                }
            },
            None => {
                // Create as indexed array with the element.
                let idx = expanded_key.parse::<usize>().unwrap_or(0);
                let mut map = std::collections::BTreeMap::new();
                map.insert(idx, value);
                state.env.insert(
                    arr_name.to_string(),
                    Variable {
                        value: VariableValue::IndexedArray(map),
                        attrs: VariableAttrs::empty(),
                    },
                );
            }
        }
        return Ok(());
    }

    // SECONDS assignment resets the shell timer.
    if target == "SECONDS" {
        if let Ok(offset) = value.parse::<u64>() {
            // `SECONDS=N` sets the timer so that $SECONDS reads as N right now.
            // We achieve this by moving shell_start_time backwards by N seconds.
            state.shell_start_time = Instant::now() - std::time::Duration::from_secs(offset);
        } else {
            state.shell_start_time = Instant::now();
        }
        return Ok(());
    }

    if let Some(var) = state.env.get(&target)
        && var.readonly()
    {
        return Err(RustBashError::Execution(format!(
            "{target}: readonly variable"
        )));
    }

    // Get attributes of target for transforms.
    let attrs = state
        .env
        .get(&target)
        .map(|v| v.attrs)
        .unwrap_or(VariableAttrs::empty());

    // INTEGER: evaluate value as arithmetic expression.
    let value = if attrs.contains(VariableAttrs::INTEGER) {
        let result = crate::interpreter::arithmetic::eval_arithmetic(&value, state)?;
        result.to_string()
    } else {
        value
    };

    // Case transforms (lowercase takes precedence if both set, but both shouldn't be).
    let value = if attrs.contains(VariableAttrs::LOWERCASE) {
        value.to_lowercase()
    } else if attrs.contains(VariableAttrs::UPPERCASE) {
        value.to_uppercase()
    } else {
        value
    };

    match state.env.get_mut(&target) {
        Some(var) => {
            match &mut var.value {
                VariableValue::IndexedArray(map) => {
                    map.insert(0, value);
                }
                VariableValue::AssociativeArray(map) => {
                    map.insert("0".to_string(), value);
                }
                VariableValue::Scalar(s) => *s = value,
            }
            var.attrs.remove(VariableAttrs::DECLARED_ONLY);
            // allexport: auto-export on every assignment
            if state.shell_opts.allexport {
                var.attrs.insert(VariableAttrs::EXPORTED);
            }
        }
        None => {
            let attrs = if state.shell_opts.allexport {
                VariableAttrs::EXPORTED
            } else {
                VariableAttrs::empty()
            };
            state.env.insert(
                target,
                Variable {
                    value: VariableValue::Scalar(value),
                    attrs,
                },
            );
        }
    }
    Ok(())
}

/// Set an array element in the interpreter state, creating the array if needed.
/// Resolves nameref before operating.
pub(crate) fn set_array_element(
    state: &mut InterpreterState,
    name: &str,
    index: usize,
    value: String,
) -> Result<(), RustBashError> {
    let target = resolve_nameref(name, state)?;
    if let Some(var) = state.env.get(&target)
        && var.readonly()
    {
        return Err(RustBashError::Execution(format!(
            "{target}: readonly variable"
        )));
    }

    // Apply attribute transforms (INTEGER, LOWERCASE, UPPERCASE).
    let attrs = state
        .env
        .get(&target)
        .map(|v| v.attrs)
        .unwrap_or(VariableAttrs::empty());
    let value = if attrs.contains(VariableAttrs::INTEGER) {
        crate::interpreter::arithmetic::eval_arithmetic(&value, state)?.to_string()
    } else {
        value
    };
    let value = if attrs.contains(VariableAttrs::LOWERCASE) {
        value.to_lowercase()
    } else if attrs.contains(VariableAttrs::UPPERCASE) {
        value.to_uppercase()
    } else {
        value
    };

    let limit = state.limits.max_array_elements;
    match state.env.get_mut(&target) {
        Some(var) => {
            match &mut var.value {
                VariableValue::IndexedArray(map) => {
                    if !map.contains_key(&index) && map.len() >= limit {
                        return Err(RustBashError::LimitExceeded {
                            limit_name: "max_array_elements",
                            limit_value: limit,
                            actual_value: map.len() + 1,
                        });
                    }
                    map.insert(index, value);
                }
                VariableValue::Scalar(_) => {
                    let mut map = BTreeMap::new();
                    map.insert(index, value);
                    var.value = VariableValue::IndexedArray(map);
                }
                VariableValue::AssociativeArray(_) => {
                    return Err(RustBashError::Execution(format!(
                        "{target}: cannot use numeric index on associative array"
                    )));
                }
            }
            var.attrs.remove(VariableAttrs::DECLARED_ONLY);
        }
        None => {
            let mut map = BTreeMap::new();
            map.insert(index, value);
            state.env.insert(
                target,
                Variable {
                    value: VariableValue::IndexedArray(map),
                    attrs: VariableAttrs::empty(),
                },
            );
        }
    }
    Ok(())
}

/// Set an associative array element. Resolves nameref before operating.
pub(crate) fn set_assoc_element(
    state: &mut InterpreterState,
    name: &str,
    key: String,
    value: String,
) -> Result<(), RustBashError> {
    let target = resolve_nameref(name, state)?;
    if let Some(var) = state.env.get(&target)
        && var.readonly()
    {
        return Err(RustBashError::Execution(format!(
            "{target}: readonly variable"
        )));
    }

    // Apply attribute transforms (INTEGER, LOWERCASE, UPPERCASE).
    let attrs = state
        .env
        .get(&target)
        .map(|v| v.attrs)
        .unwrap_or(VariableAttrs::empty());
    let value = if attrs.contains(VariableAttrs::INTEGER) {
        crate::interpreter::arithmetic::eval_arithmetic(&value, state)?.to_string()
    } else {
        value
    };
    let value = if attrs.contains(VariableAttrs::LOWERCASE) {
        value.to_lowercase()
    } else if attrs.contains(VariableAttrs::UPPERCASE) {
        value.to_uppercase()
    } else {
        value
    };

    let limit = state.limits.max_array_elements;
    match state.env.get_mut(&target) {
        Some(var) => {
            match &mut var.value {
                VariableValue::AssociativeArray(map) => {
                    if !map.contains_key(&key) && map.len() >= limit {
                        return Err(RustBashError::LimitExceeded {
                            limit_name: "max_array_elements",
                            limit_value: limit,
                            actual_value: map.len() + 1,
                        });
                    }
                    map.insert(key, value);
                }
                _ => {
                    return Err(RustBashError::Execution(format!(
                        "{target}: not an associative array"
                    )));
                }
            }
            var.attrs.remove(VariableAttrs::DECLARED_ONLY);
        }
        None => {
            return Err(RustBashError::Execution(format!(
                "{target}: not an associative array"
            )));
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

/// Resolve a nameref chain: follow NAMEREF attributes until a non-nameref variable
/// (or missing variable) is found. Returns the final target name.
/// Errors on circular references (chain longer than 10).
pub(crate) fn resolve_nameref(
    name: &str,
    state: &InterpreterState,
) -> Result<String, RustBashError> {
    let mut current = name.to_string();
    for _ in 0..10 {
        match state.env.get(&current) {
            Some(var) if var.attrs.contains(VariableAttrs::NAMEREF) => {
                current = var.value.as_scalar().to_string();
            }
            _ => return Ok(current),
        }
    }
    Err(RustBashError::Execution(format!(
        "{name}: circular name reference"
    )))
}

/// Non-failing nameref resolution: returns the resolved name, or the original
/// name if the chain is circular.
pub(crate) fn resolve_nameref_or_self(name: &str, state: &InterpreterState) -> String {
    resolve_nameref(name, state).unwrap_or_else(|_| name.to_string())
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
            current_source: "main".to_string(),
            current_source_text: String::new(),
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
            pending_cmdsub_stderr: String::new(),
            fatal_expansion_error: false,
            last_command_had_error: false,
        }
    }
}
