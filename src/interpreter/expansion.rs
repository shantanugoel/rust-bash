//! Word expansion: parameter expansion, tilde expansion, special variables,
//! IFS-based word splitting, and quoting correctness.

use crate::error::RustBashError;
use crate::interpreter::pattern;
use crate::interpreter::walker::{clone_commands, execute_program};
use crate::interpreter::{
    ExecutionCounters, InterpreterState, next_random, parse, parser_options, set_variable,
};

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
    let did_brace_expand = brace_expanded.len() > 1;

    let mut all_results = Vec::new();
    for raw in &brace_expanded {
        let sub_word = ast::Word {
            value: raw.clone(),
            loc: word.loc.clone(),
        };
        let words = expand_word_segments(&sub_word, state)?;
        let split = finalize_with_ifs_split(words, state);
        let expanded = glob_expand_words(split, state)?;
        if expanded.is_empty() && did_brace_expand {
            // Brace expansion produced this alternative; preserve it as an empty word.
            all_results.push(String::new());
        } else {
            all_results.extend(expanded);
        }
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
    let did_brace_expand = brace_expanded.len() > 1;

    let mut all_results = Vec::new();
    for raw in &brace_expanded {
        let sub_word = ast::Word {
            value: raw.clone(),
            loc: word.loc.clone(),
        };
        let words = expand_word_segments_mut(&sub_word, state)?;
        let split = finalize_with_ifs_split(words, state);
        let expanded = glob_expand_words(split, state)?;
        if expanded.is_empty() && did_brace_expand {
            all_results.push(String::new());
        } else {
            all_results.extend(expanded);
        }
    }
    Ok(all_results)
}

/// Expand a word to a single string without IFS splitting
/// (for assignments, redirections, case values, etc.).
///
/// Brace expansion is NOT applied here — assignments like `X={a,b}` keep
/// literal braces, matching bash behavior.
pub(crate) fn expand_word_to_string(
    word: &ast::Word,
    state: &InterpreterState,
) -> Result<String, RustBashError> {
    let words = expand_word_segments(word, state)?;
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

/// Mutable version of expand_word_to_string.
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
        WordPiece::SingleQuotedText(s) | WordPiece::AnsiCQuotedText(s) => {
            push_segment(words, s, true, true);
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
                push_segment(words, c, true, true);
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
            let val = crate::interpreter::arithmetic::eval_arithmetic(&expr.value, state)?;
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
            let val = resolve_parameter(parameter, state, *indirect);
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
                _ => {
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
                    let expanded = expand_raw_string(dv, state)?;
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
                    let expanded = expand_raw_string(dv, state)?;
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
                    expand_raw_string(raw, state)?
                } else {
                    "parameter null or not set".to_string()
                };
                return Err(RustBashError::Execution(format!("{param_name}: {msg}")));
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
                let expanded = expand_raw_string(av, state)?;
                push_segment(words, &expanded, in_dq, in_dq);
            }
            // If unset/null, expand to nothing
        }
        ParameterExpr::RemoveSmallestSuffixPattern {
            parameter,
            indirect,
            pattern,
        } => {
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
        ParameterExpr::RemoveLargestSuffixPattern {
            parameter,
            indirect,
            pattern,
        } => {
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
        ParameterExpr::RemoveSmallestPrefixPattern {
            parameter,
            indirect,
            pattern,
        } => {
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
        ParameterExpr::RemoveLargestPrefixPattern {
            parameter,
            indirect,
            pattern,
        } => {
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
            let result = apply_transform(&val, op);
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
        ParameterExpr::MemberKeys { .. } => {
            // Arrays not supported yet
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
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect);
            if should_use_default(&val, test_type, state, parameter) {
                let dv = if let Some(raw) = default_value {
                    expand_raw_string_mut(raw, state)?
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
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect);
            let at_empty = expand_param_value(&val, words, state, in_dq, parameter);
            Ok(at_empty)
        }
        // All other expressions delegate to immutable
        other => expand_parameter(other, words, state, in_dq),
    }
}

/// Resolve a parameter with possible mutation (e.g. $RANDOM uses next_random).
fn resolve_parameter_maybe_mut(
    parameter: &Parameter,
    state: &mut InterpreterState,
    indirect: bool,
) -> String {
    let val = match parameter {
        Parameter::Named(name) if name == "RANDOM" => next_random(state).to_string(),
        _ => resolve_parameter_direct(parameter, state),
    };
    if indirect {
        get_var(state, &val).unwrap_or_default()
    } else {
        val
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
    let mut saw_nw_delim = false;
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
            saw_nw_delim = false;
            i += 1;
        } else if is_ifs_nw(c) {
            // Non-whitespace IFS delimiter: always produces a field boundary.
            result.push(SplitWord {
                text: std::mem::take(&mut current),
                may_glob: current_may_glob,
            });
            current_may_glob = false;
            has_content = false;
            saw_nw_delim = true;
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
                saw_nw_delim = false;
            }
        } else {
            // Regular character (not IFS).
            current.push(c);
            if !glob_protected && is_glob_meta(c) {
                current_may_glob = true;
            }
            has_content = true;
            saw_nw_delim = false;
            i += 1;
        }
    }

    // Push the last field if non-empty, or a trailing empty field after non-ws delimiter.
    if has_content || !current.is_empty() {
        result.push(SplitWord {
            text: current,
            may_glob: current_may_glob,
        });
    } else if saw_nw_delim {
        result.push(SplitWord {
            text: String::new(),
            may_glob: false,
        });
    }
}

// ── Glob expansion ──────────────────────────────────────────────────

use std::path::PathBuf;

/// Expand glob metacharacters in words against the filesystem.
///
/// For each word marked `may_glob`, attempt filesystem glob expansion.
/// If matches are found, replace the word with the sorted matches.
/// If no matches are found, keep the original pattern as a literal (bash default).
fn glob_expand_words(
    words: Vec<SplitWord>,
    state: &InterpreterState,
) -> Result<Vec<String>, RustBashError> {
    let cwd = PathBuf::from(&state.cwd);
    let max = state.limits.max_glob_results;
    let mut result = Vec::new();

    for w in words {
        if !w.may_glob {
            result.push(w.text);
            continue;
        }

        match state.fs.glob(&w.text, &cwd) {
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
                // No match or error — keep pattern as literal
                result.push(w.text);
            }
        }
    }

    Ok(result)
}

// ── Transform / case helpers ────────────────────────────────────────

use brush_parser::word::ParameterTransformOp;

fn apply_transform(val: &str, op: &ParameterTransformOp) -> String {
    match op {
        ParameterTransformOp::ToUpperCase => val.to_uppercase(),
        ParameterTransformOp::ToLowerCase => val.to_lowercase(),
        ParameterTransformOp::CapitalizeInitial => uppercase_first(val),
        ParameterTransformOp::Quoted => format!("'{val}'"),
        ParameterTransformOp::ExpandEscapeSequences => val.to_string(),
        ParameterTransformOp::PromptExpand => val.to_string(),
        ParameterTransformOp::PossiblyQuoteWithArraysExpanded { .. } => val.to_string(),
        ParameterTransformOp::ToAssignmentLogic => val.to_string(),
        ParameterTransformOp::ToAttributeFlags => String::new(),
    }
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
        Parameter::NamedWithIndex { name, .. } => resolve_named_var(name, state),
        Parameter::NamedWithAllIndices { name, .. } => resolve_named_var(name, state),
    }
}

fn resolve_named_var(name: &str, state: &InterpreterState) -> String {
    // $RANDOM is handled exclusively via the mutable path
    // (resolve_parameter_maybe_mut → next_random) to use a single PRNG.
    if name == "LINENO" {
        return "0".to_string();
    }
    get_var(state, name).unwrap_or_default()
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
    state.env.get(name).map(|v| v.value.clone())
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

fn is_unset(state: &InterpreterState, parameter: &Parameter) -> bool {
    match parameter {
        Parameter::Named(name) => !state.env.contains_key(name),
        Parameter::Positional(n) => {
            if *n == 0 {
                false
            } else {
                state.positional_params.get(*n as usize - 1).is_none()
            }
        }
        Parameter::Special(_) => false,
        Parameter::NamedWithIndex { name, .. } => !state.env.contains_key(name),
        Parameter::NamedWithAllIndices { name, .. } => !state.env.contains_key(name),
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

fn expand_raw_string(raw: &str, state: &InterpreterState) -> Result<String, RustBashError> {
    let word = ast::Word {
        value: raw.to_string(),
        loc: None,
    };
    expand_word_to_string(&word, state)
}

fn expand_raw_string_mut(raw: &str, state: &mut InterpreterState) -> Result<String, RustBashError> {
    let word = ast::Word {
        value: raw.to_string(),
        loc: None,
    };
    expand_word_to_string_mut(&word, state)
}
