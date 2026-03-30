//! Word expansion: parameter expansion, tilde expansion, special variables,
//! IFS-based word splitting, and quoting correctness.

use crate::error::RustBashError;
use crate::interpreter::pattern;
use crate::interpreter::walker::{clone_commands, execute_program};
use crate::interpreter::{
    ExecutionCounters, InterpreterState, next_random, parse, parser_options, set_variable,
};

use crate::vfs::GlobOptions;
use brush_parser::ast;
use brush_parser::word::{
    Parameter, ParameterExpr, ParameterTestType, SpecialParameter, SubstringMatchKind, WordPiece,
};
use std::collections::HashMap;

// ── Word expansion intermediate types ───────────────────────────────

/// A segment of expanded text tracking quoting properties.
#[derive(Debug, Clone)]
struct Segment {
    text: String,
    /// If true, this segment came from a quoted context (single quotes, double
    /// quotes, escape sequences, or literal text) and must not be IFS-split.
    quoted: bool,
    /// If true, glob metacharacters in this segment are protected from expansion.
    /// True for single-quoted, double-quoted, and escape-sequence text.
    /// False for unquoted literal text and unquoted parameter expansions.
    glob_protected: bool,
}

/// A word being assembled from multiple segments during expansion.
type WordInProgress = Vec<Segment>;

// ── Public entry points ─────────────────────────────────────────────

/// Expand a word into a list of strings (with IFS splitting on unquoted parts).
///
/// Most expansions produce a single word. `"$@"` in double-quotes produces
/// one word per positional parameter. Unquoted `$VAR` where VAR contains
/// IFS characters may produce multiple words. Unquoted glob metacharacters
/// are expanded against the filesystem.
pub fn expand_word(
    word: &ast::Word,
    state: &InterpreterState,
) -> Result<Vec<String>, RustBashError> {
    // Brace expansion first — operates on raw word text before parsing.
    let brace_expanded =
        crate::interpreter::brace::brace_expand(&word.value, state.limits.max_brace_expansion)?;

    let mut all_results = Vec::new();
    for raw in &brace_expanded {
        let sub_word = ast::Word {
            value: raw.clone(),
            loc: word.loc.clone(),
        };
        let words = expand_word_segments(&sub_word, state)?;
        let split = finalize_with_ifs_split(words, state);
        let expanded = glob_expand_words(split, state)?;
        all_results.extend(expanded);
    }
    Ok(all_results)
}

/// Mutable variant of expand_word for expansions that assign (e.g. `${VAR:=default}`).
pub(crate) fn expand_word_mut(
    word: &ast::Word,
    state: &mut InterpreterState,
) -> Result<Vec<String>, RustBashError> {
    let brace_expanded =
        crate::interpreter::brace::brace_expand(&word.value, state.limits.max_brace_expansion)?;

    let mut all_results = Vec::new();
    for raw in &brace_expanded {
        let sub_word = ast::Word {
            value: raw.clone(),
            loc: word.loc.clone(),
        };
        let words = expand_word_segments_mut(&sub_word, state)?;
        let split = finalize_with_ifs_split(words, state);
        let expanded = glob_expand_words(split, state)?;
        all_results.extend(expanded);
    }
    Ok(all_results)
}

/// Expand a word to a single string without IFS splitting
/// (for assignments, redirections, case values, etc.).
///
/// Brace expansion is NOT applied here — assignments like `X={a,b}` keep
/// literal braces, matching bash behavior.
/// Expand a word to a single string (no word splitting or globbing).
pub(crate) fn expand_word_to_string_mut(
    word: &ast::Word,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    let words = expand_word_segments_mut(word, state)?;
    let result = finalize_no_split(words);
    let joined = result.join(" ");
    if joined.len() > state.limits.max_string_length {
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_string_length",
            limit_value: state.limits.max_string_length,
            actual_value: joined.len(),
        });
    }
    Ok(joined)
}

// ── Internal segment-based expansion ────────────────────────────────

fn expand_word_segments(
    word: &ast::Word,
    state: &InterpreterState,
) -> Result<Vec<WordInProgress>, RustBashError> {
    let options = parser_options();
    let pieces = brush_parser::word::parse(&word.value, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;

    let mut words: Vec<WordInProgress> = vec![Vec::new()];
    for piece_ws in &pieces {
        expand_word_piece(&piece_ws.piece, &mut words, state, false)?;
    }
    Ok(words)
}

fn expand_word_segments_mut(
    word: &ast::Word,
    state: &mut InterpreterState,
) -> Result<Vec<WordInProgress>, RustBashError> {
    let options = parser_options();
    let pieces = brush_parser::word::parse(&word.value, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;

    let mut words: Vec<WordInProgress> = vec![Vec::new()];
    for piece_ws in &pieces {
        expand_word_piece_mut(&piece_ws.piece, &mut words, state, false)?;
    }
    Ok(words)
}

// ── Segment helpers ─────────────────────────────────────────────────

/// Append text to the last word with the given quotedness.
/// Merges with the previous segment when quotedness matches.
/// Unquoted empty text is silently discarded; quoted empty text is preserved
/// so that `""` and `"$EMPTY"` still produce one empty word.
fn push_segment(words: &mut Vec<WordInProgress>, text: &str, quoted: bool, glob_protected: bool) {
    if text.is_empty() && !quoted {
        return;
    }
    if words.is_empty() {
        words.push(Vec::new());
    }
    let word = words.last_mut().unwrap();
    if let Some(last) = word.last_mut()
        && last.quoted == quoted
        && last.glob_protected == glob_protected
    {
        last.text.push_str(text);
        return;
    }
    word.push(Segment {
        text: text.to_string(),
        quoted,
        glob_protected,
    });
}

/// Start a new (empty) word in the word list.
fn start_new_word(words: &mut Vec<WordInProgress>) {
    words.push(Vec::new());
}

/// Execute a command substitution: parse and run the command in a subshell,
/// capture stdout, strip trailing newlines, and update `$?` in the parent.
fn execute_command_substitution(
    cmd_str: &str,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    state.counters.substitution_depth += 1;
    if state.counters.substitution_depth > state.limits.max_substitution_depth {
        let actual = state.counters.substitution_depth;
        state.counters.substitution_depth -= 1;
        return Err(RustBashError::LimitExceeded {
            limit_name: "max_substitution_depth",
            limit_value: state.limits.max_substitution_depth,
            actual_value: actual,
        });
    }

    let program = match parse(cmd_str) {
        Ok(p) => p,
        Err(e) => {
            state.counters.substitution_depth -= 1;
            return Err(e);
        }
    };

    // Create an isolated subshell state
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
        persistent_fds: state.persistent_fds.clone(),
        next_auto_fd: state.next_auto_fd,
        proc_sub_counter: state.proc_sub_counter,
        proc_sub_prealloc: HashMap::new(),
        pipe_stdin_bytes: None,
    };

    let result = execute_program(&program, &mut sub_state);

    // Fold shared counters back into parent
    state.counters.command_count = sub_state.counters.command_count;
    state.counters.output_size = sub_state.counters.output_size;
    state.counters.substitution_depth -= 1;

    let result = result?;

    // $? reflects the exit code of the substituted command
    state.last_exit_code = result.exit_code;

    // Strip trailing newlines from captured stdout
    let mut output = result.stdout;
    let trimmed_len = output.trim_end_matches('\n').len();
    output.truncate(trimmed_len);

    Ok(output)
}

// ── Piece expansion ─────────────────────────────────────────────────

/// Expand a single word piece, appending segments to the last word.
/// `in_dq` tracks whether we're inside double quotes.
/// Returns `true` if the piece was a `"$@"` expansion with zero positional params.
fn expand_word_piece(
    piece: &WordPiece,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    let mut at_empty = false;
    match piece {
        WordPiece::Text(s) => {
            // Literal text from the source — IFS-protected but glob-eligible
            // unless we are inside double quotes.
            push_segment(words, s, true, in_dq);
        }
        WordPiece::SingleQuotedText(s) => {
            push_segment(words, s, true, true);
        }
        WordPiece::AnsiCQuotedText(s) => {
            let expanded = expand_escape_sequences(s);
            push_segment(words, &expanded, true, true);
        }
        WordPiece::DoubleQuotedSequence(pieces)
        | WordPiece::GettextDoubleQuotedSequence(pieces) => {
            let word_count_before = words.len();
            let seg_count_before = words.last().map_or(0, Vec::len);
            let mut saw_at_empty = false;
            for inner in pieces {
                if expand_word_piece(&inner.piece, words, state, true)? {
                    saw_at_empty = true;
                }
            }
            // If nothing was added, ensure the quoted context still produces an
            // empty word (so `""` → one empty word, not zero words).
            // Exception: `"$@"` with zero params must produce zero words.
            if words.len() == word_count_before
                && words.last().map_or(0, Vec::len) == seg_count_before
                && !saw_at_empty
            {
                push_segment(words, "", true, true);
            }
        }
        WordPiece::EscapeSequence(s) => {
            if let Some(c) = s.strip_prefix('\\') {
                // In double quotes, only \$, \`, \", \\, and \newline are special.
                // Other \X sequences should preserve the backslash.
                if in_dq {
                    match c {
                        "$" | "`" | "\"" | "\\" | "\n" => {
                            push_segment(words, c, true, true);
                        }
                        _ => {
                            push_segment(words, s, true, true);
                        }
                    }
                } else {
                    push_segment(words, c, true, true);
                }
            } else {
                push_segment(words, s, true, true);
            }
        }
        WordPiece::TildePrefix(user) => {
            expand_tilde(user, words, state);
        }
        WordPiece::ParameterExpansion(expr) => {
            at_empty = expand_parameter(expr, words, state, in_dq)?;
        }
        // Command substitution — future phases
        WordPiece::CommandSubstitution(_) | WordPiece::BackquotedCommandSubstitution(_) => {}
        WordPiece::ArithmeticExpression(_) => {
            // Immutable path cannot evaluate arithmetic (needs mutable state).
            // Arithmetic in non-mutable context is a no-op; real usage goes
            // through expand_word_piece_mut.
        }
    }
    Ok(at_empty)
}

/// Mutable variant for pieces that may need to assign variables.
fn expand_word_piece_mut(
    piece: &WordPiece,
    words: &mut Vec<WordInProgress>,
    state: &mut InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    match piece {
        WordPiece::ParameterExpansion(expr) => {
            let at_empty = expand_parameter_mut(expr, words, state, in_dq)?;
            Ok(at_empty)
        }
        WordPiece::DoubleQuotedSequence(pieces)
        | WordPiece::GettextDoubleQuotedSequence(pieces) => {
            let word_count_before = words.len();
            let seg_count_before = words.last().map_or(0, Vec::len);
            let mut saw_at_empty = false;
            for inner in pieces {
                if expand_word_piece_mut(&inner.piece, words, state, true)? {
                    saw_at_empty = true;
                }
            }
            if words.len() == word_count_before
                && words.last().map_or(0, Vec::len) == seg_count_before
                && !saw_at_empty
            {
                push_segment(words, "", true, true);
            }
            Ok(false)
        }
        WordPiece::CommandSubstitution(cmd_str)
        | WordPiece::BackquotedCommandSubstitution(cmd_str) => {
            let output = execute_command_substitution(cmd_str, state)?;
            push_segment(words, &output, in_dq, in_dq);
            Ok(false)
        }
        WordPiece::ArithmeticExpression(expr) => {
            // Expand shell variables in the expression before arithmetic evaluation.
            let expanded = expand_arith_expression(&expr.value, state)?;
            let val = crate::interpreter::arithmetic::eval_arithmetic(&expanded, state)?;
            push_segment(words, &val.to_string(), in_dq, in_dq);
            Ok(false)
        }
        // Non-mutating pieces delegate to immutable version
        other => expand_word_piece(other, words, state, in_dq),
    }
}

// ── Tilde expansion ─────────────────────────────────────────────────

fn expand_tilde(user: &str, words: &mut Vec<WordInProgress>, state: &InterpreterState) {
    if user.is_empty() {
        // ~ → $HOME
        let home = get_var(state, "HOME").unwrap_or_default();
        push_segment(words, &home, true, true);
    } else {
        // ~user → not supported in sandbox, just output literally
        push_segment(words, "~", true, true);
        push_segment(words, user, true, true);
    }
}

// ── Parameter expansion (immutable) ─────────────────────────────────

/// Returns `true` if this was a `$@` expansion with zero positional params
/// (used to prevent `""` preservation in enclosing double quotes).
fn expand_parameter(
    expr: &ParameterExpr,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    let mut at_empty = false;
    match expr {
        ParameterExpr::Parameter {
            parameter,
            indirect,
        } => {
            check_nounset(parameter, state)?;
            let val = resolve_parameter(parameter, state, *indirect);
            at_empty = expand_param_value(&val, words, state, in_dq, parameter);
        }
        ParameterExpr::ParameterLength {
            parameter,
            indirect,
        } => {
            // ${#arr[@]} and ${#arr[*]} return element count
            match parameter {
                Parameter::Special(SpecialParameter::AllPositionalParameters {
                    concatenate: _,
                }) => {
                    push_segment(
                        words,
                        &state.positional_params.len().to_string(),
                        in_dq,
                        in_dq,
                    );
                }
                Parameter::NamedWithAllIndices { name, .. } => {
                    let values = get_array_values(name, state);
                    push_segment(words, &values.len().to_string(), in_dq, in_dq);
                }
                _ => {
                    let val = resolve_parameter(parameter, state, *indirect);
                    push_segment(words, &val.len().to_string(), in_dq, in_dq);
                }
            }
        }
        ParameterExpr::UseDefaultValues {
            parameter,
            indirect,
            test_type,
            default_value,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            if should_use_default(&val, test_type, state, parameter) {
                if let Some(dv) = default_value {
                    let expanded = expand_raw_string_ctx(dv, state, in_dq)?;
                    push_segment(words, &expanded, in_dq, in_dq);
                }
            } else {
                push_segment(words, &val, in_dq, in_dq);
            }
        }
        // AssignDefaultValues — needs mutation, handled by mut variant; here treat as UseDefault
        ParameterExpr::AssignDefaultValues {
            parameter,
            indirect,
            test_type,
            default_value,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            if should_use_default(&val, test_type, state, parameter) {
                if let Some(dv) = default_value {
                    let expanded = expand_raw_string_ctx(dv, state, in_dq)?;
                    push_segment(words, &expanded, in_dq, in_dq);
                }
            } else {
                push_segment(words, &val, in_dq, in_dq);
            }
        }
        ParameterExpr::IndicateErrorIfNullOrUnset {
            parameter,
            indirect,
            test_type,
            error_message,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            if should_use_default(&val, test_type, state, parameter) {
                let param_name = parameter_name(parameter);
                let msg = if let Some(raw) = error_message {
                    expand_raw_string_ctx(raw, state, in_dq)?
                } else {
                    "parameter null or not set".to_string()
                };
                return Err(RustBashError::ExpansionError {
                    message: format!("{param_name}: {msg}"),
                    exit_code: 127,
                });
            }
            push_segment(words, &val, in_dq, in_dq);
        }
        ParameterExpr::UseAlternativeValue {
            parameter,
            indirect,
            test_type,
            alternative_value,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            if !should_use_default(&val, test_type, state, parameter)
                && let Some(av) = alternative_value
            {
                let expanded = expand_raw_string_ctx(av, state, in_dq)?;
                push_segment(words, &expanded, in_dq, in_dq);
            }
            // If unset/null, expand to nothing
        }
        ParameterExpr::RemoveSmallestSuffixPattern {
            parameter,
            indirect,
            pattern,
        } => {
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(pat) = pattern
                            && let Some(idx) = pattern::shortest_suffix_match(v, pat)
                        {
                            v[..idx].to_string()
                        } else {
                            v.clone()
                        }
                    })
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = if let Some(pat) = pattern {
                    if let Some(idx) = pattern::shortest_suffix_match(&val, pat) {
                        val[..idx].to_string()
                    } else {
                        val
                    }
                } else {
                    val
                };
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::RemoveLargestSuffixPattern {
            parameter,
            indirect,
            pattern,
        } => {
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(pat) = pattern
                            && let Some(idx) = pattern::longest_suffix_match(v, pat)
                        {
                            v[..idx].to_string()
                        } else {
                            v.clone()
                        }
                    })
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = if let Some(pat) = pattern {
                    if let Some(idx) = pattern::longest_suffix_match(&val, pat) {
                        val[..idx].to_string()
                    } else {
                        val
                    }
                } else {
                    val
                };
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::RemoveSmallestPrefixPattern {
            parameter,
            indirect,
            pattern,
        } => {
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(pat) = pattern
                            && let Some(len) = pattern::shortest_prefix_match(v, pat)
                        {
                            v[len..].to_string()
                        } else {
                            v.clone()
                        }
                    })
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = if let Some(pat) = pattern {
                    if let Some(len) = pattern::shortest_prefix_match(&val, pat) {
                        val[len..].to_string()
                    } else {
                        val
                    }
                } else {
                    val
                };
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::RemoveLargestPrefixPattern {
            parameter,
            indirect,
            pattern,
        } => {
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(pat) = pattern
                            && let Some(len) = pattern::longest_prefix_match(v, pat)
                        {
                            v[len..].to_string()
                        } else {
                            v.clone()
                        }
                    })
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = if let Some(pat) = pattern {
                    if let Some(len) = pattern::longest_prefix_match(&val, pat) {
                        val[len..].to_string()
                    } else {
                        val
                    }
                } else {
                    val
                };
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::Substring {
            parameter,
            indirect,
            offset,
            length,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            let char_count = val.chars().count();
            let off = parse_arithmetic_value(&offset.value);
            let off = if off < 0 {
                (char_count as i64 + off).max(0) as usize
            } else {
                off as usize
            };
            let substr: String = if let Some(len_expr) = length {
                let len = parse_arithmetic_value(&len_expr.value);
                let len = if len < 0 {
                    ((char_count as i64) - (off as i64) + len).max(0) as usize
                } else {
                    len as usize
                };
                if off <= char_count {
                    val.chars().skip(off).take(len).collect()
                } else {
                    String::new()
                }
            } else if off <= char_count {
                val.chars().skip(off).collect()
            } else {
                String::new()
            };
            push_segment(words, &substr, in_dq, in_dq);
        }
        ParameterExpr::ReplaceSubstring {
            parameter,
            indirect,
            pattern: pat,
            replacement,
            match_kind,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            let repl = replacement.as_deref().unwrap_or("");
            let result = match match_kind {
                SubstringMatchKind::FirstOccurrence => {
                    if let Some((start, end)) = pattern::first_match(&val, pat) {
                        format!("{}{}{}", &val[..start], repl, &val[end..])
                    } else {
                        val
                    }
                }
                SubstringMatchKind::Anywhere => pattern::replace_all(&val, pat, repl),
                SubstringMatchKind::Prefix => {
                    if let Some(len) = pattern::longest_prefix_match(&val, pat) {
                        format!("{repl}{}", &val[len..])
                    } else {
                        val
                    }
                }
                SubstringMatchKind::Suffix => {
                    if let Some(idx) = pattern::longest_suffix_match(&val, pat) {
                        format!("{}{repl}", &val[..idx])
                    } else {
                        val
                    }
                }
            };
            push_segment(words, &result, in_dq, in_dq);
        }
        ParameterExpr::UppercaseFirstChar {
            parameter,
            indirect,
            ..
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            let result = uppercase_first(&val);
            push_segment(words, &result, in_dq, in_dq);
        }
        ParameterExpr::UppercasePattern {
            parameter,
            indirect,
            ..
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            push_segment(words, &val.to_uppercase(), in_dq, in_dq);
        }
        ParameterExpr::LowercaseFirstChar {
            parameter,
            indirect,
            ..
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            let result = lowercase_first(&val);
            push_segment(words, &result, in_dq, in_dq);
        }
        ParameterExpr::LowercasePattern {
            parameter,
            indirect,
            ..
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            push_segment(words, &val.to_lowercase(), in_dq, in_dq);
        }
        ParameterExpr::Transform {
            parameter,
            indirect,
            op,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            let var_name = parameter_name(parameter);
            let result = apply_transform(&val, op, &var_name, state);
            push_segment(words, &result, in_dq, in_dq);
        }
        ParameterExpr::VariableNames { prefix, .. } => {
            let mut names: Vec<String> = state
                .env
                .keys()
                .filter(|k| k.starts_with(prefix.as_str()))
                .cloned()
                .collect();
            names.sort();
            push_segment(words, &names.join(" "), in_dq, in_dq);
        }
        ParameterExpr::MemberKeys {
            variable_name,
            concatenate,
        } => {
            let keys = get_array_keys(variable_name, state);
            if *concatenate {
                // ${!arr[*]} — join with IFS[0], single word
                let sep = match get_var(state, "IFS") {
                    Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                    None => " ".to_string(),
                };
                push_segment(words, &keys.join(&sep), in_dq, in_dq);
            } else if keys.is_empty() {
                at_empty = true;
            } else {
                // ${!arr[@]} — each key becomes a separate word
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        start_new_word(words);
                    }
                    push_segment(words, k, in_dq, in_dq);
                }
            }
        }
    }
    Ok(at_empty)
}

/// Mutable variant that can assign defaults via `:=`.
fn expand_parameter_mut(
    expr: &ParameterExpr,
    words: &mut Vec<WordInProgress>,
    state: &mut InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    match expr {
        ParameterExpr::AssignDefaultValues {
            parameter,
            indirect,
            test_type,
            default_value,
        } => {
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect)?;
            if should_use_default(&val, test_type, state, parameter) {
                let dv = if let Some(raw) = default_value {
                    expand_raw_string_mut_ctx(raw, state, in_dq)?
                } else {
                    String::new()
                };
                if let Parameter::Named(name) = parameter {
                    set_variable(state, name, dv.clone())?;
                }
                push_segment(words, &dv, in_dq, in_dq);
            } else {
                push_segment(words, &val, in_dq, in_dq);
            }
            Ok(false)
        }
        ParameterExpr::Parameter {
            parameter,
            indirect,
        } => {
            check_nounset(parameter, state)?;
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect)?;
            let at_empty = expand_param_value(&val, words, state, in_dq, parameter);
            Ok(at_empty)
        }
        // All other expressions delegate to immutable
        other => expand_parameter(other, words, state, in_dq),
    }
}

/// Resolve a parameter with possible mutation (e.g. $RANDOM uses next_random).
/// Returns Result to propagate circular nameref errors.
fn resolve_parameter_maybe_mut(
    parameter: &Parameter,
    state: &mut InterpreterState,
    indirect: bool,
) -> Result<String, RustBashError> {
    // Check for circular namerefs on Named parameters.
    if let Parameter::Named(name) = parameter {
        crate::interpreter::resolve_nameref(name, state)?;
    }
    let val = match parameter {
        Parameter::Named(name) if name == "RANDOM" => next_random(state).to_string(),
        _ => resolve_parameter_direct(parameter, state),
    };
    if indirect {
        Ok(get_var(state, &val).unwrap_or_default())
    } else {
        Ok(val)
    }
}

// ── $@ / $* expansion ───────────────────────────────────────────────

/// Expand a parameter value into word segments, handling $@ and $* split semantics.
/// Returns `true` if this was a `$@` expansion with zero positional params.
fn expand_param_value(
    val: &str,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
    parameter: &Parameter,
) -> bool {
    match parameter {
        Parameter::Special(SpecialParameter::AllPositionalParameters { concatenate }) => {
            if *concatenate {
                // $* — join with first char of IFS.
                // IFS unset → default space; IFS="" → no separator.
                let sep = match get_var(state, "IFS") {
                    Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                    None => " ".to_string(),
                };
                let joined = state.positional_params.join(&sep);
                push_segment(words, &joined, in_dq, in_dq);
                false
            } else if state.positional_params.is_empty() {
                // $@ with zero params — signal to DQ handler to not create empty word.
                true
            } else {
                // $@ — each positional parameter becomes a separate word.
                // In double quotes ("$@"): each param is a quoted word.
                // Outside quotes ($@): each param is an unquoted word (subject to IFS split).
                for (i, param) in state.positional_params.iter().enumerate() {
                    if i > 0 {
                        start_new_word(words);
                    }
                    push_segment(words, param, in_dq, in_dq);
                }
                false
            }
        }
        Parameter::NamedWithAllIndices { name, concatenate } => {
            let values = get_array_values(name, state);
            if *concatenate {
                // ${arr[*]} — join with first char of IFS
                let sep = match get_var(state, "IFS") {
                    Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                    None => " ".to_string(),
                };
                let joined = values.join(&sep);
                push_segment(words, &joined, in_dq, in_dq);
                false
            } else if values.is_empty() {
                // ${arr[@]} with zero elements — signal empty like $@
                true
            } else {
                // ${arr[@]} — each element becomes a separate word (in dq)
                for (i, v) in values.iter().enumerate() {
                    if i > 0 {
                        start_new_word(words);
                    }
                    push_segment(words, v, in_dq, in_dq);
                }
                false
            }
        }
        _ => {
            push_segment(words, val, in_dq, in_dq);
            false
        }
    }
}

// ── IFS word splitting ──────────────────────────────────────────────

/// Get the IFS value from state, defaulting to space+tab+newline.
fn get_ifs(state: &InterpreterState) -> String {
    get_var(state, "IFS").unwrap_or_else(|| " \t\n".to_string())
}

/// A word after IFS splitting, carrying glob eligibility metadata.
struct SplitWord {
    text: String,
    /// True if the word may contain unquoted glob metacharacters.
    may_glob: bool,
}

/// Finalize expanded words by performing IFS splitting on unquoted segments.
fn finalize_with_ifs_split(words: Vec<WordInProgress>, state: &InterpreterState) -> Vec<SplitWord> {
    let ifs = get_ifs(state);
    let mut result = Vec::new();
    for word in words {
        ifs_split_word(&word, &ifs, &mut result);
    }
    result
}

/// Finalize expanded words by concatenating segments without IFS splitting.
fn finalize_no_split(words: Vec<WordInProgress>) -> Vec<String> {
    words
        .into_iter()
        .map(|segments| segments.into_iter().map(|s| s.text).collect::<String>())
        .collect()
}

/// Check whether a character is a glob metacharacter.
fn is_glob_meta(c: char) -> bool {
    matches!(c, '*' | '?' | '[')
}

/// IFS-split a single expanded word (represented as segments) into result words.
///
/// The algorithm flattens segments to character-level quotedness, then scans
/// through splitting only on unquoted IFS characters.
fn ifs_split_word(word: &[Segment], ifs: &str, result: &mut Vec<SplitWord>) {
    // Flatten segments to (char, quoted, glob_protected) triples.
    let chars: Vec<(char, bool, bool)> = word
        .iter()
        .flat_map(|s| s.text.chars().map(move |c| (c, s.quoted, s.glob_protected)))
        .collect();

    if chars.is_empty() {
        // An empty word with at least one quoted segment → produce one empty word.
        if word.iter().any(|s| s.quoted) {
            result.push(SplitWord {
                text: String::new(),
                may_glob: false,
            });
        }
        return;
    }

    // Fast path: entirely quoted → single word, no splitting.
    if chars.iter().all(|(_, q, _)| *q) {
        let s: String = chars.iter().map(|(c, _, _)| c).collect();
        let may_glob = chars.iter().any(|(c, _, gp)| !gp && is_glob_meta(*c));
        result.push(SplitWord { text: s, may_glob });
        return;
    }

    // Classify IFS characters.
    let ifs_ws: Vec<char> = ifs
        .chars()
        .filter(|c| matches!(c, ' ' | '\t' | '\n'))
        .collect();
    let ifs_non_ws: Vec<char> = ifs
        .chars()
        .filter(|c| !matches!(c, ' ' | '\t' | '\n'))
        .collect();

    let is_ifs_ws = |c: char| ifs_ws.contains(&c);
    let is_ifs_nw = |c: char| ifs_non_ws.contains(&c);

    let len = chars.len();
    let mut current = String::new();
    let mut current_may_glob = false;
    let mut has_content = false;
    let mut i = 0;

    // Skip leading unquoted IFS whitespace.
    while i < len {
        let (c, quoted, _) = chars[i];
        if !quoted && is_ifs_ws(c) {
            i += 1;
        } else {
            break;
        }
    }

    while i < len {
        let (c, quoted, glob_protected) = chars[i];
        if quoted {
            current.push(c);
            if !glob_protected && is_glob_meta(c) {
                current_may_glob = true;
            }
            has_content = true;
            i += 1;
        } else if is_ifs_nw(c) {
            // Non-whitespace IFS delimiter: always produces a field boundary.
            result.push(SplitWord {
                text: std::mem::take(&mut current),
                may_glob: current_may_glob,
            });
            current_may_glob = false;
            has_content = false;
            i += 1;
            // Skip trailing IFS whitespace after delimiter.
            while i < len && !chars[i].1 && is_ifs_ws(chars[i].0) {
                i += 1;
            }
        } else if is_ifs_ws(c) {
            // Run of unquoted IFS whitespace.
            while i < len && !chars[i].1 && is_ifs_ws(chars[i].0) {
                i += 1;
            }
            // If followed by unquoted non-ws IFS char, this ws is absorbed into that delimiter.
            if i < len && !chars[i].1 && is_ifs_nw(chars[i].0) {
                continue;
            }
            // Standalone whitespace delimiter.
            if has_content || !current.is_empty() {
                result.push(SplitWord {
                    text: std::mem::take(&mut current),
                    may_glob: current_may_glob,
                });
                current_may_glob = false;
                has_content = false;
            }
        } else {
            // Regular character (not IFS).
            current.push(c);
            if !glob_protected && is_glob_meta(c) {
                current_may_glob = true;
            }
            has_content = true;
            i += 1;
        }
    }

    // Push the last field if non-empty. Trailing non-whitespace IFS delimiters
    // do NOT produce a trailing empty field (bash behavior).
    if has_content || !current.is_empty() {
        result.push(SplitWord {
            text: current,
            may_glob: current_may_glob,
        });
    }
}

// ── Glob expansion ──────────────────────────────────────────────────

use std::path::PathBuf;

/// Expand glob metacharacters in words against the filesystem.
///
/// For each word marked `may_glob`, attempt filesystem glob expansion.
/// Behavior depends on shopt options: nullglob, failglob, dotglob,
/// nocaseglob, and globstar. When `set -f` (noglob) is active, all
/// glob expansion is skipped and patterns pass through as literals.
fn glob_expand_words(
    words: Vec<SplitWord>,
    state: &InterpreterState,
) -> Result<Vec<String>, RustBashError> {
    // noglob: skip all filename expansion
    if state.shell_opts.noglob {
        return Ok(words.into_iter().map(|w| w.text).collect());
    }

    let cwd = PathBuf::from(&state.cwd);
    let max = state.limits.max_glob_results;
    let opts = GlobOptions {
        dotglob: state.shopt_opts.dotglob,
        nocaseglob: state.shopt_opts.nocaseglob,
        globstar: state.shopt_opts.globstar,
    };
    let mut result = Vec::new();

    for w in words {
        if !w.may_glob {
            result.push(w.text);
            continue;
        }

        match state.fs.glob_with_opts(&w.text, &cwd, &opts) {
            Ok(matches) if !matches.is_empty() => {
                if matches.len() > max {
                    return Err(RustBashError::LimitExceeded {
                        limit_name: "max_glob_results",
                        limit_value: max,
                        actual_value: matches.len(),
                    });
                }
                for p in &matches {
                    result.push(p.to_string_lossy().into_owned());
                }
            }
            _ => {
                if state.shopt_opts.failglob {
                    return Err(RustBashError::FailGlob {
                        pattern: w.text.clone(),
                    });
                }
                if state.shopt_opts.nullglob {
                    // nullglob: pattern expands to nothing
                    continue;
                }
                // Default: keep pattern as literal
                result.push(w.text);
            }
        }
    }

    Ok(result)
}

// ── Transform / case helpers ────────────────────────────────────────

use brush_parser::word::ParameterTransformOp;

fn apply_transform(
    val: &str,
    op: &ParameterTransformOp,
    var_name: &str,
    state: &InterpreterState,
) -> String {
    match op {
        ParameterTransformOp::ToUpperCase => val.to_uppercase(),
        ParameterTransformOp::ToLowerCase => val.to_lowercase(),
        ParameterTransformOp::CapitalizeInitial => uppercase_first(val),
        ParameterTransformOp::Quoted => shell_quote(val),
        ParameterTransformOp::ExpandEscapeSequences => expand_escape_sequences(val),
        ParameterTransformOp::PromptExpand => expand_prompt_sequences(val, state),
        ParameterTransformOp::PossiblyQuoteWithArraysExpanded { .. } => shell_quote(val),
        ParameterTransformOp::ToAssignmentLogic => format_assignment(var_name, state),
        ParameterTransformOp::ToAttributeFlags => format_attribute_flags(var_name, state),
    }
}

/// Shell-quote a value so it can be safely reused as input (@Q).
/// Empty strings → `''`. Strings without single quotes → `'val'`.
/// Strings with single quotes → `$'...'` with escaping.
fn shell_quote(val: &str) -> String {
    if val.is_empty() {
        return "''".to_string();
    }
    // Use $'...' if the string contains single quotes or non-printable chars
    let needs_dollar_quote = val.chars().any(|c| c == '\'' || c.is_ascii_control());
    if !needs_dollar_quote {
        return format!("'{val}'");
    }
    // Use $'...' notation for strings with single quotes
    let mut out = String::from("$'");
    for ch in val.chars() {
        match ch {
            '\'' => out.push_str("\\'"),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\x07' => out.push_str("\\a"),
            '\x08' => out.push_str("\\b"),
            '\x0C' => out.push_str("\\f"),
            '\x0B' => out.push_str("\\v"),
            '\x1B' => out.push_str("\\E"),
            c if c.is_ascii_control() => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Expand backslash escape sequences in a string (@E).
fn expand_escape_sequences(val: &str) -> String {
    let mut result = String::new();
    let chars: Vec<char> = val.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                'r' => result.push('\r'),
                'a' => result.push('\x07'),
                'b' => result.push('\x08'),
                'f' => result.push('\x0C'),
                'v' => result.push('\x0B'),
                'e' | 'E' => result.push('\x1B'),
                '\\' => result.push('\\'),
                '\'' => result.push('\''),
                '"' => result.push('"'),
                'x' => {
                    // \xHH — hex escape
                    let mut hex = String::new();
                    while hex.len() < 2 && i + 1 < chars.len() && chars[i + 1].is_ascii_hexdigit() {
                        i += 1;
                        hex.push(chars[i]);
                    }
                    if hex.is_empty() {
                        // No hex digits followed — preserve as literal \x
                        result.push('\\');
                        result.push('x');
                    } else if let Ok(n) = u32::from_str_radix(&hex, 16)
                        && let Some(c) = char::from_u32(n)
                    {
                        result.push(c);
                    }
                    // Invalid codepoints (e.g. surrogates \uD800) silently produce nothing, matching bash.
                }
                'u' => {
                    // \uHHHH — unicode escape (up to 4 hex digits)
                    let mut hex = String::new();
                    while hex.len() < 4 && i + 1 < chars.len() && chars[i + 1].is_ascii_hexdigit() {
                        i += 1;
                        hex.push(chars[i]);
                    }
                    if hex.is_empty() {
                        result.push('\\');
                        result.push('u');
                    } else if let Ok(n) = u32::from_str_radix(&hex, 16)
                        && let Some(c) = char::from_u32(n)
                    {
                        result.push(c);
                    }
                }
                'U' => {
                    // \UHHHHHHHH — unicode escape (up to 8 hex digits)
                    let mut hex = String::new();
                    while hex.len() < 8 && i + 1 < chars.len() && chars[i + 1].is_ascii_hexdigit() {
                        i += 1;
                        hex.push(chars[i]);
                    }
                    if hex.is_empty() {
                        result.push('\\');
                        result.push('U');
                    } else if let Ok(n) = u32::from_str_radix(&hex, 16)
                        && let Some(c) = char::from_u32(n)
                    {
                        result.push(c);
                    }
                }
                '0'..='7' => {
                    // Octal escape: \0NNN (leading zero, up to 3 more digits)
                    // or \NNN (1-7, up to 2 more digits)
                    let first_digit = chars[i].to_digit(8).unwrap_or(0);
                    let max_extra = if chars[i] == '0' { 3 } else { 2 };
                    let mut val_octal = first_digit;
                    let mut count = 0;
                    while count < max_extra
                        && i + 1 < chars.len()
                        && chars[i + 1] >= '0'
                        && chars[i + 1] <= '7'
                    {
                        i += 1;
                        val_octal = val_octal * 8 + chars[i].to_digit(8).unwrap_or(0);
                        count += 1;
                    }
                    if let Some(c) = char::from_u32(val_octal) {
                        result.push(c);
                    }
                }
                other => {
                    result.push('\\');
                    result.push(other);
                }
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }
    result
}

/// Expand prompt escape sequences (@P).
fn expand_prompt_sequences(val: &str, state: &InterpreterState) -> String {
    let mut result = String::new();
    let chars: Vec<char> = val.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                'u' => {
                    result.push_str(&get_var(state, "USER").unwrap_or_else(|| "user".to_string()));
                }
                'h' => {
                    let hostname =
                        get_var(state, "HOSTNAME").unwrap_or_else(|| "localhost".to_string());
                    // \h is short hostname (up to first dot)
                    result.push_str(hostname.split('.').next().unwrap_or(&hostname));
                }
                'H' => {
                    result.push_str(
                        &get_var(state, "HOSTNAME").unwrap_or_else(|| "localhost".to_string()),
                    );
                }
                'w' => {
                    let cwd = &state.cwd;
                    let home = get_var(state, "HOME").unwrap_or_default();
                    if !home.is_empty() && cwd.starts_with(&home) {
                        result.push('~');
                        result.push_str(&cwd[home.len()..]);
                    } else {
                        result.push_str(cwd);
                    }
                }
                'W' => {
                    let cwd = &state.cwd;
                    if cwd == "/" {
                        result.push('/');
                    } else {
                        result.push_str(cwd.rsplit('/').next().unwrap_or(cwd));
                    }
                }
                'd' => {
                    // \d — "Weekday Month Day" in current locale
                    result.push_str("Mon Jan 01");
                }
                't' => {
                    // \t — HH:MM:SS (24-hour)
                    result.push_str("00:00:00");
                }
                'T' => {
                    // \T — HH:MM:SS (12-hour)
                    result.push_str("12:00:00");
                }
                '@' => {
                    // \@ — HH:MM AM/PM
                    result.push_str("12:00 AM");
                }
                'A' => {
                    // \A — HH:MM (24-hour)
                    result.push_str("00:00");
                }
                'n' => result.push('\n'),
                'r' => result.push('\r'),
                'a' => result.push('\x07'),
                'e' => result.push('\x1B'),
                's' => {
                    result.push_str(&state.shell_name);
                }
                'v' | 'V' => {
                    result.push_str("5.0");
                }
                '#' => {
                    result.push_str(&state.counters.command_count.to_string());
                }
                '$' => {
                    // \$ — '#' if uid is 0, else '$'
                    result.push('$');
                }
                '[' | ']' => {
                    // Non-printing character delimiters — empty in output
                }
                '\\' => result.push('\\'),
                other => {
                    result.push('\\');
                    result.push(other);
                }
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }
    result
}

/// Format a variable as an assignment statement (@A).
fn format_assignment(name: &str, state: &InterpreterState) -> String {
    use crate::interpreter::{VariableAttrs, VariableValue};
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let var = match state.env.get(&resolved) {
        Some(v) => v,
        None => return String::new(),
    };

    let mut flags = String::from("declare ");
    let mut flag_chars = String::new();
    match &var.value {
        VariableValue::IndexedArray(_) => flag_chars.push('a'),
        VariableValue::AssociativeArray(_) => flag_chars.push('A'),
        VariableValue::Scalar(_) => {}
    }
    if var.attrs.contains(VariableAttrs::INTEGER) {
        flag_chars.push('i');
    }
    if var.attrs.contains(VariableAttrs::LOWERCASE) {
        flag_chars.push('l');
    }
    if var.attrs.contains(VariableAttrs::NAMEREF) {
        flag_chars.push('n');
    }
    if var.attrs.contains(VariableAttrs::READONLY) {
        flag_chars.push('r');
    }
    if var.attrs.contains(VariableAttrs::UPPERCASE) {
        flag_chars.push('u');
    }
    if var.attrs.contains(VariableAttrs::EXPORTED) {
        flag_chars.push('x');
    }

    if flag_chars.is_empty() {
        flags.push_str("-- ");
    } else {
        flags.push('-');
        flags.push_str(&flag_chars);
        flags.push(' ');
    }

    match &var.value {
        VariableValue::Scalar(s) => {
            format!("{flags}{resolved}='{s}'")
        }
        VariableValue::IndexedArray(map) => {
            let elements: Vec<String> = map.iter().map(|(k, v)| format!("[{k}]=\"{v}\"")).collect();
            format!("{flags}{resolved}=({})", elements.join(" "))
        }
        VariableValue::AssociativeArray(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let elements: Vec<String> = keys
                .iter()
                .map(|k| format!("[{k}]=\"{}\"", map[*k]))
                .collect();
            format!("{flags}{resolved}=({})", elements.join(" "))
        }
    }
}

/// Return attribute flags as a string (@a).
fn format_attribute_flags(name: &str, state: &InterpreterState) -> String {
    use crate::interpreter::{VariableAttrs, VariableValue};
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let var = match state.env.get(&resolved) {
        Some(v) => v,
        None => return String::new(),
    };
    let mut flags = String::new();
    match &var.value {
        VariableValue::IndexedArray(_) => flags.push('a'),
        VariableValue::AssociativeArray(_) => flags.push('A'),
        VariableValue::Scalar(_) => {}
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
    flags
}

fn uppercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => {
            let mut result = c.to_uppercase().to_string();
            result.extend(chars);
            result
        }
    }
}

fn lowercase_first(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => {
            let mut result = c.to_lowercase().to_string();
            result.extend(chars);
            result
        }
    }
}

// ── Parameter resolution ────────────────────────────────────────────

/// Check if `set -u` (nounset) should produce an error for this parameter.
/// Returns an error if nounset is enabled and the parameter is unset.
/// Special parameters ($@, $*, $#, $?, etc.) are always exempt.
fn check_nounset(parameter: &Parameter, state: &InterpreterState) -> Result<(), RustBashError> {
    if !state.shell_opts.nounset {
        return Ok(());
    }
    // Special parameters are always OK
    if matches!(parameter, Parameter::Special(_)) {
        return Ok(());
    }
    if is_unset(state, parameter) {
        let name = parameter_name(parameter);
        return Err(RustBashError::Execution(format!(
            "{name}: unbound variable"
        )));
    }
    Ok(())
}

fn resolve_parameter(parameter: &Parameter, state: &InterpreterState, indirect: bool) -> String {
    let val = resolve_parameter_direct(parameter, state);
    if indirect {
        get_var(state, &val).unwrap_or_default()
    } else {
        val
    }
}

fn resolve_parameter_direct(parameter: &Parameter, state: &InterpreterState) -> String {
    match parameter {
        Parameter::Named(name) => resolve_named_var(name, state),
        Parameter::Positional(n) => {
            if *n == 0 {
                state.shell_name.clone()
            } else {
                state
                    .positional_params
                    .get(*n as usize - 1)
                    .cloned()
                    .unwrap_or_default()
            }
        }
        Parameter::Special(sp) => resolve_special(sp, state),
        Parameter::NamedWithIndex { name, index } => resolve_array_element(name, index, state),
        Parameter::NamedWithAllIndices { name, concatenate } => {
            // For resolve_parameter_direct, join all values into a single string.
            // The actual multi-word expansion for [@] is handled in expand_param_value.
            resolve_all_elements(name, *concatenate, state)
        }
    }
}

/// Resolve `${arr[index]}` — look up a specific element of an array variable.
fn resolve_array_element(name: &str, index: &str, state: &InterpreterState) -> String {
    // Handle call-stack pseudo-arrays before checking env.
    if let Some(val) = resolve_call_stack_element(name, index, state) {
        return val;
    }
    use crate::interpreter::VariableValue;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let Some(var) = state.env.get(&resolved) else {
        return String::new();
    };
    match &var.value {
        VariableValue::IndexedArray(map) => {
            let idx = simple_arith_eval(index, state) as usize;
            map.get(&idx).cloned().unwrap_or_default()
        }
        VariableValue::AssociativeArray(map) => map.get(index).cloned().unwrap_or_default(),
        VariableValue::Scalar(s) => {
            let idx = simple_arith_eval(index, state) as usize;
            if idx == 0 { s.clone() } else { String::new() }
        }
    }
}

/// Resolve `${FUNCNAME[i]}`, `${BASH_SOURCE[i]}`, `${BASH_LINENO[i]}` from the call stack.
/// Returns `None` if `name` is not a call-stack array, so the caller falls through to env.
fn resolve_call_stack_element(name: &str, index: &str, state: &InterpreterState) -> Option<String> {
    match name {
        "FUNCNAME" | "BASH_SOURCE" | "BASH_LINENO" => {}
        _ => return None,
    }
    let idx = simple_arith_eval(index, state) as usize;
    // The call stack is ordered innermost-last; bash indexes 0 = current (innermost).
    // Build a reversed view: index 0 = top of stack, last = bottom ("main").
    let len = state.call_stack.len();
    if idx >= len {
        return Some(String::new());
    }
    let frame_idx = len - 1 - idx;
    let frame = &state.call_stack[frame_idx];
    Some(match name {
        "FUNCNAME" => frame.func_name.clone(),
        "BASH_SOURCE" => frame.source.clone(),
        "BASH_LINENO" => frame.lineno.to_string(),
        _ => String::new(),
    })
}

/// Simple arithmetic evaluation for array indices in immutable contexts.
/// Handles integer literals, variable names, and simple expressions.
fn simple_arith_eval(expr: &str, state: &InterpreterState) -> i64 {
    let trimmed = expr.trim();
    // Try as integer literal
    if let Ok(n) = trimmed.parse::<i64>() {
        return n;
    }
    // Try as variable name
    if trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return read_var_immutable(state, trimmed);
    }
    // For complex expressions, return 0 — full arithmetic eval requires &mut
    0
}

/// Read a variable as i64 (immutable — for use in expansion.rs contexts).
fn read_var_immutable(state: &InterpreterState, name: &str) -> i64 {
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    state
        .env
        .get(&resolved)
        .map(|v| v.value.as_scalar().parse::<i64>().unwrap_or(0))
        .unwrap_or(0)
}

/// Resolve all elements of an array, joined into a single string.
/// `concatenate=true` → `[*]` (join with IFS[0]), `concatenate=false` → `[@]` (join with space).
fn resolve_all_elements(name: &str, concatenate: bool, state: &InterpreterState) -> String {
    // Handle call-stack pseudo-arrays.
    if let Some(vals) = get_call_stack_values(name, state) {
        let sep = if concatenate {
            match get_var(state, "IFS") {
                Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                None => " ".to_string(),
            }
        } else {
            " ".to_string()
        };
        return vals.join(&sep);
    }
    use crate::interpreter::VariableValue;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let Some(var) = state.env.get(&resolved) else {
        return String::new();
    };
    let values: Vec<&str> = match &var.value {
        VariableValue::IndexedArray(map) => map.values().map(|s| s.as_str()).collect(),
        VariableValue::AssociativeArray(map) => map.values().map(|s| s.as_str()).collect(),
        VariableValue::Scalar(s) => {
            if s.is_empty() {
                vec![]
            } else {
                vec![s.as_str()]
            }
        }
    };
    if concatenate {
        let sep = match get_var(state, "IFS") {
            Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
            None => " ".to_string(),
        };
        values.join(&sep)
    } else {
        values.join(" ")
    }
}

/// Get all values of call-stack pseudo-arrays as a Vec of owned Strings.
/// Returns `None` if `name` is not a call-stack array.
fn get_call_stack_values(name: &str, state: &InterpreterState) -> Option<Vec<String>> {
    match name {
        "FUNCNAME" => Some(
            state
                .call_stack
                .iter()
                .rev()
                .map(|f| f.func_name.clone())
                .collect(),
        ),
        "BASH_SOURCE" => Some(
            state
                .call_stack
                .iter()
                .rev()
                .map(|f| f.source.clone())
                .collect(),
        ),
        "BASH_LINENO" => Some(
            state
                .call_stack
                .iter()
                .rev()
                .map(|f| f.lineno.to_string())
                .collect(),
        ),
        _ => None,
    }
}

/// Returns the individual element values for a parameter if it represents an
/// array expansion (`[@]` or `[*]` or `$@` / `$*`).  Returns `None` for scalar
/// parameters so the caller can fall through to the normal scalar path.  When
/// `Some` is returned, the bool indicates whether the values should be
/// concatenated (`[*]` / `$*`) or kept separate (`[@]` / `$@`).
fn get_vectorized_values(
    parameter: &Parameter,
    state: &InterpreterState,
    indirect: bool,
) -> Option<(Vec<String>, bool)> {
    let _ = indirect; // indirect not yet relevant for array expansion
    match parameter {
        Parameter::NamedWithAllIndices { name, concatenate } => {
            Some((get_array_values(name, state), *concatenate))
        }
        Parameter::Special(SpecialParameter::AllPositionalParameters { concatenate }) => {
            Some((state.positional_params.clone(), *concatenate))
        }
        _ => None,
    }
}

/// Push vectorized operation results into `words`, handling `[@]` vs `[*]`
/// semantics (separate words vs IFS-joined).
fn push_vectorized(
    results: Vec<String>,
    concatenate: bool,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
) {
    if concatenate {
        let sep = match get_var(state, "IFS") {
            Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
            None => " ".to_string(),
        };
        let joined = results.join(&sep);
        push_segment(words, &joined, in_dq, in_dq);
    } else {
        for (i, v) in results.iter().enumerate() {
            if i > 0 {
                start_new_word(words);
            }
            push_segment(words, v, in_dq, in_dq);
        }
    }
}

/// Get all values of an array variable as a Vec.
fn get_array_values(name: &str, state: &InterpreterState) -> Vec<String> {
    // Handle call-stack pseudo-arrays first.
    if let Some(vals) = get_call_stack_values(name, state) {
        return vals;
    }
    use crate::interpreter::VariableValue;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let Some(var) = state.env.get(&resolved) else {
        return Vec::new();
    };
    match &var.value {
        VariableValue::IndexedArray(map) => map.values().cloned().collect(),
        VariableValue::AssociativeArray(map) => map.values().cloned().collect(),
        VariableValue::Scalar(s) => {
            if s.is_empty() {
                vec![]
            } else {
                vec![s.clone()]
            }
        }
    }
}

/// Get keys/indices of an array variable.
fn get_array_keys(name: &str, state: &InterpreterState) -> Vec<String> {
    // Handle call-stack pseudo-arrays first.
    if let Some(vals) = get_call_stack_values(name, state) {
        return (0..vals.len()).map(|i| i.to_string()).collect();
    }
    use crate::interpreter::VariableValue;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let Some(var) = state.env.get(&resolved) else {
        return Vec::new();
    };
    match &var.value {
        VariableValue::IndexedArray(map) => map.keys().map(|k| k.to_string()).collect(),
        VariableValue::AssociativeArray(map) => map.keys().cloned().collect(),
        VariableValue::Scalar(s) => {
            if s.is_empty() {
                vec![]
            } else {
                vec!["0".to_string()]
            }
        }
    }
}

fn resolve_named_var(name: &str, state: &InterpreterState) -> String {
    // $RANDOM is handled exclusively via the mutable path
    // (resolve_parameter_maybe_mut → next_random) to use a single PRNG.
    match name {
        "LINENO" => return state.current_lineno.to_string(),
        "SECONDS" => return state.shell_start_time.elapsed().as_secs().to_string(),
        "_" => return state.last_argument.clone(),
        "PPID" => {
            return get_var(state, "PPID").unwrap_or_else(|| "1".to_string());
        }
        "UID" => {
            return get_var(state, "UID").unwrap_or_else(|| "1000".to_string());
        }
        "EUID" => {
            return get_var(state, "EUID").unwrap_or_else(|| "1000".to_string());
        }
        "BASHPID" => {
            return get_var(state, "BASHPID").unwrap_or_else(|| "1".to_string());
        }
        "SHELLOPTS" => return compute_shellopts(state),
        "BASHOPTS" => return compute_bashopts(state),
        "MACHTYPE" => return state.machtype.clone(),
        "HOSTTYPE" => return state.hosttype.clone(),
        "FUNCNAME" | "BASH_SOURCE" | "BASH_LINENO" => {
            return resolve_call_stack_scalar(name, state);
        }
        _ => {}
    }
    get_var(state, name).unwrap_or_default()
}

/// Compute `SHELLOPTS` — colon-separated list of enabled `set -o` options.
fn compute_shellopts(state: &InterpreterState) -> String {
    let mut opts = Vec::new();
    if state.shell_opts.errexit {
        opts.push("errexit");
    }
    if state.shell_opts.nounset {
        opts.push("nounset");
    }
    if state.shell_opts.pipefail {
        opts.push("pipefail");
    }
    if state.shell_opts.xtrace {
        opts.push("xtrace");
    }
    opts.sort_unstable();
    opts.join(":")
}

/// Compute `BASHOPTS` — colon-separated list of enabled `shopt` options.
fn compute_bashopts(state: &InterpreterState) -> String {
    let mut opts = Vec::new();
    if state.shopt_opts.dotglob {
        opts.push("dotglob");
    }
    if state.shopt_opts.expand_aliases {
        opts.push("expand_aliases");
    }
    if state.shopt_opts.extglob {
        opts.push("extglob");
    }
    if state.shopt_opts.failglob {
        opts.push("failglob");
    }
    if state.shopt_opts.globskipdots {
        opts.push("globskipdots");
    }
    if state.shopt_opts.globstar {
        opts.push("globstar");
    }
    if state.shopt_opts.lastpipe {
        opts.push("lastpipe");
    }
    if state.shopt_opts.nocaseglob {
        opts.push("nocaseglob");
    }
    if state.shopt_opts.nocasematch {
        opts.push("nocasematch");
    }
    if state.shopt_opts.nullglob {
        opts.push("nullglob");
    }
    if state.shopt_opts.xpg_echo {
        opts.push("xpg_echo");
    }
    opts.join(":")
}

/// Resolve `FUNCNAME`, `BASH_SOURCE`, or `BASH_LINENO` as a scalar
/// (returns value at index 0, i.e. current/innermost frame).
fn resolve_call_stack_scalar(name: &str, state: &InterpreterState) -> String {
    if state.call_stack.is_empty() {
        return String::new();
    }
    let frame = &state.call_stack[state.call_stack.len() - 1];
    match name {
        "FUNCNAME" => frame.func_name.clone(),
        "BASH_SOURCE" => frame.source.clone(),
        "BASH_LINENO" => frame.lineno.to_string(),
        _ => String::new(),
    }
}

fn resolve_special(sp: &SpecialParameter, state: &InterpreterState) -> String {
    match sp {
        SpecialParameter::LastExitStatus => state.last_exit_code.to_string(),
        SpecialParameter::PositionalParameterCount => state.positional_params.len().to_string(),
        SpecialParameter::AllPositionalParameters { concatenate } => {
            if *concatenate {
                // IFS unset → default space; IFS="" → no separator.
                let sep = match get_var(state, "IFS") {
                    Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                    None => " ".to_string(),
                };
                state.positional_params.join(&sep)
            } else {
                state.positional_params.join(" ")
            }
        }
        SpecialParameter::ProcessId => "1".to_string(),
        SpecialParameter::LastBackgroundProcessId => String::new(),
        SpecialParameter::ShellName => state.shell_name.clone(),
        SpecialParameter::CurrentOptionFlags => {
            let mut flags = String::new();
            if state.shell_opts.errexit {
                flags.push('e');
            }
            if state.shell_opts.nounset {
                flags.push('u');
            }
            if state.shell_opts.xtrace {
                flags.push('x');
            }
            flags
        }
    }
}

fn get_var(state: &InterpreterState, name: &str) -> Option<String> {
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    state
        .env
        .get(&resolved)
        .map(|v| v.value.as_scalar().to_string())
}

fn should_use_default(
    val: &str,
    test_type: &ParameterTestType,
    state: &InterpreterState,
    parameter: &Parameter,
) -> bool {
    match test_type {
        ParameterTestType::UnsetOrNull => val.is_empty() || is_unset(state, parameter),
        ParameterTestType::Unset => is_unset(state, parameter),
    }
}

/// Names that are always "set" because they are dynamically computed.
fn is_dynamic_special(name: &str) -> bool {
    matches!(
        name,
        "LINENO"
            | "SECONDS"
            | "_"
            | "PPID"
            | "UID"
            | "EUID"
            | "BASHPID"
            | "SHELLOPTS"
            | "BASHOPTS"
            | "MACHTYPE"
            | "HOSTTYPE"
            | "FUNCNAME"
            | "BASH_SOURCE"
            | "BASH_LINENO"
    )
}

fn is_unset(state: &InterpreterState, parameter: &Parameter) -> bool {
    match parameter {
        Parameter::Named(name) => {
            if is_dynamic_special(name) {
                return false;
            }
            let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
            !state.env.contains_key(&resolved)
        }
        Parameter::Positional(n) => {
            if *n == 0 {
                false
            } else {
                state.positional_params.get(*n as usize - 1).is_none()
            }
        }
        Parameter::Special(_) => false,
        Parameter::NamedWithIndex { name, index } => {
            if is_dynamic_special(name) {
                return false;
            }
            let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
            match state.env.get(&resolved) {
                None => true,
                Some(var) => {
                    use crate::interpreter::VariableValue;
                    match &var.value {
                        VariableValue::IndexedArray(map) => {
                            let idx = simple_arith_eval(index, state) as usize;
                            !map.contains_key(&idx)
                        }
                        VariableValue::AssociativeArray(map) => !map.contains_key(index.as_str()),
                        VariableValue::Scalar(_) => {
                            let idx = simple_arith_eval(index, state) as usize;
                            idx != 0
                        }
                    }
                }
            }
        }
        Parameter::NamedWithAllIndices { name, .. } => {
            if is_dynamic_special(name) {
                return false;
            }
            let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
            !state.env.contains_key(&resolved)
        }
    }
}

fn parameter_name(parameter: &Parameter) -> String {
    match parameter {
        Parameter::Named(name) => name.clone(),
        Parameter::Positional(n) => n.to_string(),
        Parameter::Special(sp) => match sp {
            SpecialParameter::LastExitStatus => "?".to_string(),
            SpecialParameter::PositionalParameterCount => "#".to_string(),
            SpecialParameter::AllPositionalParameters { concatenate } => {
                if *concatenate {
                    "*".to_string()
                } else {
                    "@".to_string()
                }
            }
            SpecialParameter::ProcessId => "$".to_string(),
            SpecialParameter::LastBackgroundProcessId => "!".to_string(),
            SpecialParameter::ShellName => "0".to_string(),
            SpecialParameter::CurrentOptionFlags => "-".to_string(),
        },
        Parameter::NamedWithIndex { name, index } => format!("{name}[{index}]"),
        Parameter::NamedWithAllIndices { name, .. } => name.clone(),
    }
}

/// Parse a simple integer from an arithmetic expression string.
fn parse_arithmetic_value(expr: &str) -> i64 {
    let trimmed = expr.trim();
    trimmed.parse::<i64>().unwrap_or(0)
}

// ── Raw string expansion (for default/alternative values) ───────────

fn expand_raw_string_ctx(
    raw: &str,
    state: &InterpreterState,
    in_dq: bool,
) -> Result<String, RustBashError> {
    let options = parser_options();
    let pieces = brush_parser::word::parse(raw, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;

    let mut words: Vec<WordInProgress> = vec![Vec::new()];
    for piece_ws in &pieces {
        expand_word_piece(&piece_ws.piece, &mut words, state, in_dq)?;
    }
    let result = finalize_no_split(words);
    Ok(result.join(" "))
}

fn expand_raw_string_mut_ctx(
    raw: &str,
    state: &mut InterpreterState,
    in_dq: bool,
) -> Result<String, RustBashError> {
    let options = parser_options();
    let pieces = brush_parser::word::parse(raw, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;

    let mut words: Vec<WordInProgress> = vec![Vec::new()];
    for piece_ws in &pieces {
        expand_word_piece_mut(&piece_ws.piece, &mut words, state, in_dq)?;
    }
    let result = finalize_no_split(words);
    Ok(result.join(" "))
}

/// Expand shell variables inside an arithmetic expression before evaluation.
/// This handles cases like `$((${zero}11))` where `zero=0` should yield `011`.
fn expand_arith_expression(
    expr: &str,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    // If the expression contains no shell expansion markers, return as-is.
    if !expr.contains('$') && !expr.contains('`') {
        return Ok(expr.to_string());
    }
    // Parse the expression as a shell word and expand it.
    let word = ast::Word {
        value: expr.to_string(),
        loc: None,
    };
    expand_word_to_string_mut(&word, state)
}
