//! Word expansion: parameter expansion, tilde expansion, special variables,
//! IFS-based word splitting, and quoting correctness.

use crate::error::RustBashError;
use crate::interpreter::pattern;
use crate::interpreter::walker::{clone_commands, execute_program};
use crate::interpreter::{
    ExecutionCounters, InterpreterState, next_random, parse, parser_options, set_assoc_element,
    set_variable,
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
    /// True for synthetic empty fields created by unquoted `$@` / `${arr[@]}`
    /// with non-whitespace IFS delimiters.
    synthetic_empty: bool,
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
    validate_length_transform_syntax(&word.value)?;
    validate_empty_slice_syntax(&word.value)?;
    let options = parser_options();
    let rewritten = rewrite_special_case_word_syntax(&word.value, state);
    let assignment_like = expand_assignment_like_tilde_bug(&rewritten, state);
    let pieces = brush_parser::word::parse(&assignment_like, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;
    if pieces
        .iter()
        .all(|piece| matches!(piece.piece, WordPiece::Text(_)))
        && assignment_like.starts_with("${")
        && assignment_like.ends_with('}')
    {
        validate_unparsed_dollar_brace_word(&assignment_like)?;
    }

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
    validate_length_transform_syntax(&word.value)?;
    validate_empty_slice_syntax(&word.value)?;
    let options = parser_options();
    let rewritten = rewrite_special_case_word_syntax(&word.value, state);
    let assignment_like = expand_assignment_like_tilde_bug(&rewritten, state);
    let pieces = brush_parser::word::parse(&assignment_like, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;
    if pieces
        .iter()
        .all(|piece| matches!(piece.piece, WordPiece::Text(_)))
        && assignment_like.starts_with("${")
        && assignment_like.ends_with('}')
    {
        validate_unparsed_dollar_brace_word(&assignment_like)?;
    }

    let mut words: Vec<WordInProgress> = vec![Vec::new()];
    for piece_ws in &pieces {
        expand_word_piece_mut(&piece_ws.piece, &mut words, state, false)?;
    }
    Ok(words)
}

fn expand_assignment_like_tilde_bug(word: &str, state: &InterpreterState) -> String {
    if word.contains(['"', '\'', '\\', '$', '`']) {
        return word.to_string();
    }

    let Some((name, value)) = word.split_once('=') else {
        return word.to_string();
    };

    if !is_assignment_like_name(name) || !value.starts_with('~') {
        return word.to_string();
    }

    let rest = &value[1..];
    if !rest.is_empty() && !rest.starts_with('/') {
        return word.to_string();
    }

    let home = get_var(state, "HOME").unwrap_or_default();
    if home.is_empty() {
        return word.to_string();
    }

    format!("{name}={home}{rest}")
}

fn rewrite_special_case_word_syntax(word: &str, state: &InterpreterState) -> String {
    let rewritten = word.replace("///}", "//\\//}");
    let rewritten = rewrite_assoc_indirect_attr_special_cases(&rewritten, state);
    rewrite_ambiguous_substring_ternaries(&rewritten)
}

fn rewrite_assoc_indirect_attr_special_cases(word: &str, state: &InterpreterState) -> String {
    let mut out = String::with_capacity(word.len());
    let mut i = 0usize;
    while let Some(rel_start) = word[i..].find("${!") {
        let start = i + rel_start;
        out.push_str(&word[i..start]);
        let mut j = start + 3;
        while j < word.len() {
            let ch = word.as_bytes()[j];
            if ch.is_ascii_alphanumeric() || ch == b'_' {
                j += 1;
            } else {
                break;
            }
        }
        if j == start + 3 {
            out.push_str("${!");
            i = j;
            continue;
        }

        let name = &word[start + 3..j];
        let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
        let is_assoc = state.env.get(&resolved).is_some_and(|var| {
            matches!(
                var.value,
                crate::interpreter::VariableValue::AssociativeArray(_)
            )
        });

        let rest = &word[j..];
        if is_assoc && (rest.starts_with("@a}") || rest.starts_with("[@]@a}")) {
            i = j + if rest.starts_with("@a}") { 3 } else { 6 };
            continue;
        }

        out.push_str("${!");
        out.push_str(name);
        i = j;
    }
    out.push_str(&word[i..]);
    out
}

fn rewrite_ambiguous_substring_ternaries(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    let mut i = 0usize;

    while let Some(rel_start) = word[i..].find("${") {
        let start = i + rel_start;
        out.push_str(&word[i..start]);

        let Some((body, end)) = take_parameter_body(word, start + 2) else {
            out.push_str(&word[start..]);
            return out;
        };

        out.push_str("${");
        out.push_str(&rewrite_parameter_body_ambiguous_slice(body));
        out.push('}');
        i = end + 1;
    }

    out.push_str(&word[i..]);
    out
}

fn take_parameter_body(word: &str, start: usize) -> Option<(&str, usize)> {
    let bytes = word.as_bytes();
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&word[start..i], i));
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn rewrite_parameter_body_ambiguous_slice(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
        i += 1;
    }
    if i == 0 || i >= bytes.len() || bytes[i] != b':' {
        return body.to_string();
    }

    let mut bracket_depth = 0usize;
    let mut colon_positions = Vec::new();
    let mut question_seen = false;
    for (offset, ch) in body[i + 1..].char_indices() {
        match ch {
            '[' => bracket_depth += 1,
            ']' if bracket_depth > 0 => bracket_depth -= 1,
            '?' if bracket_depth == 0 => question_seen = true,
            ':' if bracket_depth == 0 => colon_positions.push(i + 1 + offset),
            _ => {}
        }
    }

    if !question_seen || colon_positions.len() < 2 {
        return body.to_string();
    }

    let split = *colon_positions.last().unwrap();
    let param = &body[..i];
    let offset_expr = &body[i + 1..split];
    let length_expr = &body[split + 1..];
    format!("{param}:$(({offset_expr})):{}", length_expr.trim_start())
}

fn is_assignment_like_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
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
        && !last.synthetic_empty
    {
        last.text.push_str(text);
        return;
    }
    word.push(Segment {
        text: text.to_string(),
        quoted,
        glob_protected,
        synthetic_empty: false,
    });
}

fn push_synthetic_empty_segment(words: &mut Vec<WordInProgress>) {
    if words.is_empty() {
        words.push(Vec::new());
    }
    words.last_mut().unwrap().push(Segment {
        text: String::new(),
        quoted: false,
        glob_protected: false,
        synthetic_empty: true,
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
        temp_binding_scopes: Vec::new(),
        in_function_depth: 0,
        traps: HashMap::new(),
        in_trap: false,
        errexit_suppressed: 0,
        stdin_offset: 0,
        dir_stack: state.dir_stack.clone(),
        command_hash: state.command_hash.clone(),
        aliases: state.aliases.clone(),
        current_lineno: state.current_lineno,
        current_source: state.current_source.clone(),
        current_source_text: state.current_source_text.clone(),
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
        pending_cmdsub_stderr: String::new(),
        fatal_expansion_error: false,
        last_command_had_error: false,
    };

    let result = execute_program(&program, &mut sub_state);

    // Fold shared counters back into parent
    state.counters.command_count = sub_state.counters.command_count;
    state.counters.output_size = sub_state.counters.output_size;
    state.counters.substitution_depth -= 1;

    let mut result = result?;

    // $? reflects the exit code of the substituted command
    state.last_exit_code = result.exit_code;

    // In bash, stderr from command substitution passes through to the parent.
    // Accumulate it so the enclosing command can include it in its ExecResult.
    if !result.stderr.is_empty() {
        state.pending_cmdsub_stderr.push_str(&result.stderr);
    }

    // Strip trailing newlines from captured stdout.
    // When a command produced binary output, decode it lossily so command
    // substitution still preserves the visible text portion instead of
    // collapsing to the empty string.
    let mut output = if let Some(bytes) = result.stdout_bytes.take() {
        crate::shell_bytes::decode_shell_bytes(&bytes)
    } else {
        result.stdout
    };
    let trimmed_len = output.trim_end_matches('\n').len();
    output.truncate(trimmed_len);

    Ok(output)
}

fn validate_unparsed_dollar_brace_word(word: &str) -> Result<(), RustBashError> {
    if !(word.starts_with("${") && word.ends_with('}')) {
        return Ok(());
    }

    let body = &word[2..word.len() - 1];
    if body.starts_with('|') {
        return Err(bad_substitution_error(word));
    }
    let Some(end) = consume_parameter_reference_end(body.as_bytes()) else {
        return Ok(());
    };
    let rest = &body[end..];
    if rest.starts_with('&') || rest.starts_with(';') || rest.starts_with('|') {
        return Err(bad_substitution_error(word));
    }

    Ok(())
}

fn bad_substitution_error(word: &str) -> RustBashError {
    RustBashError::ExpansionError {
        message: format!("{word}: bad substitution"),
        exit_code: 1,
        should_exit: true,
    }
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
        WordPiece::TildeExpansion(expr) => {
            expand_tilde(expr, words, state);
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

fn expand_tilde(
    expr: &brush_parser::word::TildeExpr,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
) {
    use brush_parser::word::TildeExpr;
    match expr {
        TildeExpr::Home => {
            let home = get_var(state, "HOME").unwrap_or_default();
            push_segment(words, &home, true, true);
        }
        TildeExpr::WorkingDir => {
            let pwd = get_var(state, "PWD").unwrap_or_default();
            push_segment(words, &pwd, true, true);
        }
        TildeExpr::OldWorkingDir => {
            let oldpwd = get_var(state, "OLDPWD").unwrap_or_default();
            push_segment(words, &oldpwd, true, true);
        }
        TildeExpr::UserHome(user) => {
            if user == "root" {
                push_segment(words, "/root", true, true);
            } else {
                // ~user → not supported in sandbox, output literally
                push_segment(words, "~", true, true);
                push_segment(words, user, true, true);
            }
        }
        TildeExpr::NthDirFromTopOfDirStack { .. }
        | TildeExpr::NthDirFromBottomOfDirStack { .. } => {
            // Directory stack tilde expansion not yet supported
            push_segment(words, "~", true, true);
        }
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
    validate_expr_parameter(expr)?;
    validate_indirect_reference(expr, state)?;
    let mut at_empty = false;
    let ext = state.shopt_opts.extglob;
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
            check_nounset(parameter, state)?;
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
                    push_segment(words, &string_length(&val, state).to_string(), in_dq, in_dq);
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
            let use_default = should_use_default_for_parameter(
                parameter, *indirect, &val, test_type, state, in_dq,
            );
            if use_default {
                if let Some(dv) = default_value {
                    expand_raw_into_words(dv, words, state, in_dq)?;
                }
            } else {
                push_expanded_parameter_value(parameter, *indirect, &val, words, state, in_dq);
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
            let use_default = should_use_default_for_parameter(
                parameter, *indirect, &val, test_type, state, in_dq,
            );
            if use_default {
                if let Some(dv) = default_value {
                    // AssignDefaultValues collapses to a single string for both
                    // assignment and expansion (bash behavior).
                    let expanded = expand_raw_string_ctx(dv, state, in_dq)?;
                    push_segment(words, &expanded, in_dq, in_dq);
                }
            } else {
                push_expanded_parameter_value(parameter, *indirect, &val, words, state, in_dq);
            }
        }
        ParameterExpr::IndicateErrorIfNullOrUnset {
            parameter,
            indirect,
            test_type,
            error_message,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            let use_default = should_use_default_for_parameter(
                parameter, *indirect, &val, test_type, state, in_dq,
            );
            if use_default {
                let param_name = parameter_name(parameter);
                let msg = if let Some(raw) = error_message {
                    expand_raw_string_ctx(raw, state, in_dq)?
                } else {
                    "parameter null or not set".to_string()
                };
                return Err(RustBashError::ExpansionError {
                    message: format!("{param_name}: {msg}"),
                    exit_code: 1,
                    should_exit: true,
                });
            }
            push_expanded_parameter_value(parameter, *indirect, &val, words, state, in_dq);
        }
        ParameterExpr::UseAlternativeValue {
            parameter,
            indirect,
            test_type,
            alternative_value,
        } => {
            let val = resolve_parameter(parameter, state, *indirect);
            let use_default = should_use_default_for_parameter(
                parameter, *indirect, &val, test_type, state, in_dq,
            );
            if !use_default && let Some(av) = alternative_value {
                expand_raw_into_words(av, words, state, in_dq)?;
            } else if !*indirect
                && vectorized_parameter_words(parameter, state, in_dq)
                    .is_some_and(|vals| vals.is_empty())
            {
                at_empty = true;
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
                let pat_expanded = pattern
                    .as_ref()
                    .map(|p| expand_pattern_string(p, state))
                    .transpose()?;
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(ref pat) = pat_expanded
                            && let Some(idx) = pattern::shortest_suffix_match_ext(v, pat, ext)
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
                    let pat = expand_pattern_string(pat, state)?;
                    if let Some(idx) = pattern::shortest_suffix_match_ext(&val, &pat, ext) {
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
                let pat_expanded = pattern
                    .as_ref()
                    .map(|p| expand_pattern_string(p, state))
                    .transpose()?;
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(ref pat) = pat_expanded
                            && let Some(idx) = pattern::longest_suffix_match_ext(v, pat, ext)
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
                    let pat = expand_pattern_string(pat, state)?;
                    if let Some(idx) = pattern::longest_suffix_match_ext(&val, &pat, ext) {
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
                let pat_expanded = pattern
                    .as_ref()
                    .map(|p| expand_pattern_string(p, state))
                    .transpose()?;
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(ref pat) = pat_expanded
                            && let Some(len) = pattern::shortest_prefix_match_ext(v, pat, ext)
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
                    let pat = expand_pattern_string(pat, state)?;
                    if let Some(len) = pattern::shortest_prefix_match_ext(&val, &pat, ext) {
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
                let pat_expanded = pattern
                    .as_ref()
                    .map(|p| expand_pattern_string(p, state))
                    .transpose()?;
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        if let Some(ref pat) = pat_expanded
                            && let Some(len) = pattern::longest_prefix_match_ext(v, pat, ext)
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
                    let pat = expand_pattern_string(pat, state)?;
                    if let Some(len) = pattern::longest_prefix_match_ext(&val, &pat, ext) {
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
            check_nounset(parameter, state)?;
            // Check if this is an array/positional parameter needing element-level slicing
            if let Parameter::Special(SpecialParameter::AllPositionalParameters { concatenate }) =
                parameter
            {
                let off_raw = parse_arithmetic_value(&offset.value);
                let (values, start) = positional_slice_values_and_start(state, off_raw);
                let sliced: Vec<String> = if let Some(len_expr) = length {
                    let len_raw = parse_arithmetic_value(&len_expr.value);
                    if len_raw < 0 {
                        return Err(negative_substring_length_error(&len_expr.value));
                    }
                    values
                        .into_iter()
                        .skip(start)
                        .take(len_raw as usize)
                        .collect()
                } else {
                    values.into_iter().skip(start).collect()
                };
                push_vectorized(sliced, *concatenate, words, state, in_dq);
            } else if let Some((values, concatenate)) =
                get_vectorized_values(parameter, state, *indirect)
            {
                let elem_count = values.len() as i64;
                let off_raw = parse_arithmetic_value(&offset.value);
                let off = if off_raw < 0 {
                    (elem_count + off_raw).max(0) as usize
                } else {
                    off_raw as usize
                };
                let sliced: Vec<String> = if let Some(len_expr) = length {
                    let len_raw = parse_arithmetic_value(&len_expr.value);
                    if len_raw < 0 {
                        return Err(negative_substring_length_error(&len_expr.value));
                    }
                    values
                        .into_iter()
                        .skip(off)
                        .take(len_raw as usize)
                        .collect()
                } else {
                    values.into_iter().skip(off).collect()
                };
                push_vectorized(sliced, concatenate, words, state, in_dq);
            } else {
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
        }
        ParameterExpr::ReplaceSubstring {
            parameter,
            indirect,
            pattern: raw_pat,
            replacement: raw_repl,
            match_kind,
        } => {
            check_nounset(parameter, state)?;
            let (pattern_src, replacement_src) =
                normalize_patsub_slashes(raw_pat, raw_repl.as_deref());
            let pat = expand_pattern_string(pattern_src, state)?;
            let repl_expanded = replacement_src
                .as_ref()
                .map(|r| expand_replacement_string(r, state))
                .transpose()?;
            let repl = repl_expanded.as_deref().unwrap_or("");
            let byte_mode = is_byte_locale(state);
            let do_replace = |val: &str| -> String {
                match match_kind {
                    SubstringMatchKind::FirstOccurrence => {
                        if let Some((start, end)) =
                            pattern::first_match_ext_with_mode(val, &pat, ext, byte_mode)
                        {
                            format!("{}{}{}", &val[..start], repl, &val[end..])
                        } else {
                            val.to_string()
                        }
                    }
                    SubstringMatchKind::Anywhere => {
                        pattern::replace_all_ext_with_mode(val, &pat, repl, ext, byte_mode)
                    }
                    SubstringMatchKind::Prefix => {
                        if let Some(len) =
                            pattern::longest_prefix_match_ext_with_mode(val, &pat, ext, byte_mode)
                        {
                            if byte_mode {
                                let suffix = String::from_utf8_lossy(&val.as_bytes()[len..]);
                                format!("{repl}{suffix}")
                            } else {
                                format!("{repl}{}", &val[len..])
                            }
                        } else {
                            val.to_string()
                        }
                    }
                    SubstringMatchKind::Suffix => {
                        if let Some(idx) =
                            pattern::longest_suffix_match_ext_with_mode(val, &pat, ext, byte_mode)
                        {
                            if byte_mode {
                                let prefix = String::from_utf8_lossy(&val.as_bytes()[..idx]);
                                format!("{prefix}{repl}")
                            } else {
                                format!("{}{repl}", &val[..idx])
                            }
                        } else {
                            val.to_string()
                        }
                    }
                }
            };
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values.iter().map(|v| do_replace(v)).collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = do_replace(&val);
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::UppercaseFirstChar {
            parameter,
            indirect,
            pattern,
        } => {
            let pat = pattern
                .as_ref()
                .map(|p| expand_pattern_string(p, state))
                .transpose()?
                .filter(|p| !p.is_empty());
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| uppercase_first_matching(v, pat.as_deref(), ext))
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = uppercase_first_matching(&val, pat.as_deref(), ext);
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::UppercasePattern {
            parameter,
            indirect,
            pattern,
        } => {
            let pat = pattern
                .as_ref()
                .map(|p| expand_pattern_string(p, state))
                .transpose()?
                .filter(|p| !p.is_empty());
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| uppercase_matching(v, pat.as_deref(), ext))
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                push_segment(
                    words,
                    &uppercase_matching(&val, pat.as_deref(), ext),
                    in_dq,
                    in_dq,
                );
            }
        }
        ParameterExpr::LowercaseFirstChar {
            parameter,
            indirect,
            pattern,
        } => {
            let pat = pattern
                .as_ref()
                .map(|p| expand_pattern_string(p, state))
                .transpose()?
                .filter(|p| !p.is_empty());
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| lowercase_first_matching(v, pat.as_deref(), ext))
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = lowercase_first_matching(&val, pat.as_deref(), ext);
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::LowercasePattern {
            parameter,
            indirect,
            pattern,
        } => {
            let pat = pattern
                .as_ref()
                .map(|p| expand_pattern_string(p, state))
                .transpose()?
                .filter(|p| !p.is_empty());
            if let Some((values, concatenate)) = get_vectorized_values(parameter, state, *indirect)
            {
                let results: Vec<String> = values
                    .iter()
                    .map(|v| lowercase_matching(v, pat.as_deref(), ext))
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                push_segment(
                    words,
                    &lowercase_matching(&val, pat.as_deref(), ext),
                    in_dq,
                    in_dq,
                );
            }
        }
        ParameterExpr::Transform {
            parameter,
            indirect,
            op,
        } => {
            check_nounset(parameter, state)?;
            let transform_name = transform_target_name(parameter, *indirect, state);
            let scalar_defined = !parameter_scalar_is_unset(parameter, *indirect, state);
            let variable_exists = parameter_variable_exists(parameter, *indirect, state);
            if let Some((mut values, concatenate)) =
                get_vectorized_values(parameter, state, *indirect)
            {
                if values.is_empty() && !concatenate {
                    at_empty = true;
                    return Ok(at_empty);
                }
                if parameter_is_associative_array(parameter, *indirect, state) {
                    values.reverse();
                }
                let results: Vec<String> = values
                    .iter()
                    .map(|v| {
                        apply_transform(
                            v,
                            op,
                            transform_name.as_deref(),
                            scalar_defined,
                            variable_exists,
                            state,
                        )
                    })
                    .collect();
                push_vectorized(results, concatenate, words, state, in_dq);
            } else {
                let val = resolve_parameter(parameter, state, *indirect);
                let result = apply_transform(
                    &val,
                    op,
                    transform_name.as_deref(),
                    scalar_defined,
                    variable_exists,
                    state,
                );
                push_segment(words, &result, in_dq, in_dq);
            }
        }
        ParameterExpr::VariableNames {
            prefix,
            concatenate,
        } => {
            let mut names: Vec<String> = state
                .env
                .keys()
                .filter(|k| k.starts_with(prefix.as_str()))
                .cloned()
                .collect();
            names.sort();
            if *concatenate {
                // ${!prefix*} — join with IFS[0], single word
                let sep = match get_var(state, "IFS") {
                    Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                    None => " ".to_string(),
                };
                push_segment(words, &names.join(&sep), in_dq, in_dq);
            } else if names.is_empty() {
                at_empty = true;
            } else {
                // ${!prefix@} — each name becomes a separate word
                for (i, name) in names.iter().enumerate() {
                    if i > 0 {
                        start_new_word(words);
                    }
                    push_segment(words, name, in_dq, in_dq);
                }
            }
        }
        ParameterExpr::MemberKeys {
            variable_name,
            concatenate,
        } => {
            let keys = get_array_keys(variable_name, state);
            if *concatenate {
                // Bash joins ${!arr[*]} with spaces when IFS is empty.
                let sep = match get_var(state, "IFS") {
                    Some(s) if s.is_empty() => " ".to_string(),
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
    validate_expr_parameter(expr)?;
    validate_indirect_reference(expr, state)?;
    match expr {
        ParameterExpr::UseDefaultValues {
            parameter,
            indirect,
            test_type,
            default_value,
        } => {
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect)?;
            let use_default = should_use_default_for_parameter_mut(
                parameter, *indirect, &val, test_type, state, in_dq,
            )?;
            if use_default {
                if let Some(raw) = default_value {
                    expand_raw_into_words_mut(raw, words, state, in_dq)?;
                }
            } else {
                push_expanded_parameter_value(parameter, *indirect, &val, words, state, in_dq);
            }
            Ok(false)
        }
        ParameterExpr::AssignDefaultValues {
            parameter,
            indirect,
            test_type,
            default_value,
        } => {
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect)?;
            let use_default = should_use_default_for_parameter_mut(
                parameter, *indirect, &val, test_type, state, in_dq,
            )?;
            if use_default {
                // AssignDefaultValues collapses to a single string.
                let dv = if let Some(raw) = default_value {
                    expand_raw_string_mut_ctx(raw, state, in_dq)?
                } else {
                    String::new()
                };
                assign_default_to_parameter(parameter, *indirect, &dv, state)?;
                push_segment(words, &dv, in_dq, in_dq);
            } else {
                push_expanded_parameter_value(parameter, *indirect, &val, words, state, in_dq);
            }
            Ok(false)
        }
        ParameterExpr::IndicateErrorIfNullOrUnset {
            parameter,
            indirect,
            test_type,
            error_message,
        } => {
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect)?;
            let use_default = should_use_default_for_parameter_mut(
                parameter, *indirect, &val, test_type, state, in_dq,
            )?;
            if use_default {
                let param_name = parameter_name(parameter);
                let msg = if let Some(raw) = error_message {
                    expand_raw_string_mut_ctx(raw, state, in_dq)?
                } else {
                    "parameter null or not set".to_string()
                };
                return Err(RustBashError::ExpansionError {
                    message: format!("{param_name}: {msg}"),
                    exit_code: 1,
                    should_exit: true,
                });
            }
            push_expanded_parameter_value(parameter, *indirect, &val, words, state, in_dq);
            Ok(false)
        }
        ParameterExpr::UseAlternativeValue {
            parameter,
            indirect,
            test_type,
            alternative_value,
        } => {
            let val = resolve_parameter_maybe_mut(parameter, state, *indirect)?;
            let use_default = should_use_default_for_parameter_mut(
                parameter, *indirect, &val, test_type, state, in_dq,
            )?;
            if !use_default && let Some(raw) = alternative_value {
                expand_raw_into_words_mut(raw, words, state, in_dq)?;
            } else if !*indirect
                && vectorized_parameter_words(parameter, state, in_dq)
                    .is_some_and(|vals| vals.is_empty())
            {
                return Ok(true);
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
        ParameterExpr::Substring {
            parameter,
            indirect,
            offset,
            length,
        } => {
            check_nounset(parameter, state)?;
            if let Parameter::Special(SpecialParameter::AllPositionalParameters { concatenate }) =
                parameter
            {
                let expanded_off = expand_arith_expression(&offset.value, state)?;
                let off_raw =
                    crate::interpreter::arithmetic::eval_arithmetic(&expanded_off, state)?;
                let (values, start) = positional_slice_values_and_start(state, off_raw);
                let sliced: Vec<String> = if let Some(len_expr) = length {
                    let expanded_len = expand_arith_expression(&len_expr.value, state)?;
                    let len_raw =
                        crate::interpreter::arithmetic::eval_arithmetic(&expanded_len, state)?;
                    if len_raw < 0 {
                        return Err(negative_substring_length_error(&len_expr.value));
                    }
                    values
                        .into_iter()
                        .skip(start)
                        .take(len_raw as usize)
                        .collect()
                } else {
                    values.into_iter().skip(start).collect()
                };
                push_vectorized(sliced, *concatenate, words, state, in_dq);
            } else if let Some((_, concatenate)) =
                get_vectorized_values(parameter, state, *indirect)
            {
                // Array slicing with full arithmetic evaluation.

                // Get key-value pairs for proper sparse-array handling.
                let kv_pairs = get_array_kv_pairs(parameter, state);
                let max_key = kv_pairs.last().map(|(k, _)| *k).unwrap_or(0) as i64;

                // Evaluate offset as arithmetic.
                let expanded_off = expand_arith_expression(&offset.value, state)?;
                let off_raw =
                    crate::interpreter::arithmetic::eval_arithmetic(&expanded_off, state)?;

                // Compute the key-based threshold for indexed arrays.
                // For negative offsets: threshold = max_key + 1 + offset.
                let compute_threshold = |raw: i64| -> Option<usize> {
                    if raw < 0 {
                        let t = max_key.checked_add(1).and_then(|v| v.checked_add(raw));
                        match t {
                            Some(v) if v >= 0 => Some(v as usize),
                            _ => None,
                        }
                    } else {
                        Some(raw as usize)
                    }
                };

                let sliced: Vec<String> = if let Some(len_expr) = length {
                    let expanded_len = expand_arith_expression(&len_expr.value, state)?;
                    let len_raw =
                        crate::interpreter::arithmetic::eval_arithmetic(&expanded_len, state)?;
                    if len_raw < 0 {
                        return Err(negative_substring_length_error(&len_expr.value));
                    }
                    let len = len_raw as usize;
                    match compute_threshold(off_raw) {
                        None => Vec::new(),
                        Some(threshold) => kv_pairs
                            .into_iter()
                            .filter(|(k, _)| *k >= threshold)
                            .map(|(_, v)| v)
                            .take(len)
                            .collect(),
                    }
                } else {
                    // No length — take all from offset.
                    match compute_threshold(off_raw) {
                        None => Vec::new(),
                        Some(threshold) => kv_pairs
                            .into_iter()
                            .filter(|(k, _)| *k >= threshold)
                            .map(|(_, v)| v)
                            .collect(),
                    }
                };
                push_vectorized(sliced, concatenate, words, state, in_dq);
            } else {
                // Scalar substring.
                let val = resolve_parameter_maybe_mut(parameter, state, *indirect)?;
                let char_count = val.chars().count();
                let expanded_off = expand_arith_expression(&offset.value, state)?;
                let off = crate::interpreter::arithmetic::eval_arithmetic(&expanded_off, state)?;
                let off = if off < 0 {
                    (char_count as i64 + off).max(0) as usize
                } else {
                    off as usize
                };
                let substr: String = if let Some(len_expr) = length {
                    let expanded_len = expand_arith_expression(&len_expr.value, state)?;
                    let len =
                        crate::interpreter::arithmetic::eval_arithmetic(&expanded_len, state)?;
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
            Ok(false)
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
    if let Parameter::Named(name) = parameter
        && let Err(_) = crate::interpreter::resolve_nameref(name, state)
    {
        // Circular nameref: set exit code 1, return empty
        // (bash prints a warning to stderr here — we silently fail to avoid
        // bypassing VFS with eprintln!)
        state.last_exit_code = 1;
        return Ok(String::new());
    }
    let val = match parameter {
        Parameter::Named(name) if name == "RANDOM" => next_random(state).to_string(),
        Parameter::NamedWithIndex { name, index } => resolve_array_element_mut(name, index, state)?,
        _ => resolve_parameter_direct(parameter, state),
    };
    if indirect {
        Ok(resolve_indirect_value(&val, state))
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
                let ifs_val = get_var(state, "IFS");
                let ifs_empty = matches!(&ifs_val, Some(s) if s.is_empty());
                if !in_dq && ifs_empty {
                    // Unquoted $* with IFS='': each param is a separate word (like $@)
                    if state.positional_params.is_empty() {
                        return true;
                    }
                    for (i, param) in state.positional_params.iter().enumerate() {
                        if i > 0 {
                            start_new_word(words);
                        }
                        push_segment(words, param, false, false);
                    }
                    return false;
                }
                let sep = match ifs_val {
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
                    if !in_dq && param.is_empty() && ifs_preserves_unquoted_empty(state) {
                        push_synthetic_empty_segment(words);
                    } else {
                        push_segment(words, param, in_dq, in_dq);
                    }
                }
                false
            }
        }
        Parameter::NamedWithAllIndices { name, concatenate } => {
            let values = get_array_values(name, state);
            if *concatenate {
                // ${arr[*]} — join with first char of IFS
                let ifs_val = get_var(state, "IFS");
                let ifs_empty = matches!(&ifs_val, Some(s) if s.is_empty());
                if !in_dq && ifs_empty {
                    // Unquoted ${arr[*]} with IFS='': each element separate (like ${arr[@]})
                    if values.is_empty() {
                        return true;
                    }
                    for (i, v) in values.iter().enumerate() {
                        if i > 0 {
                            start_new_word(words);
                        }
                        push_segment(words, v, false, false);
                    }
                    return false;
                }
                let sep = match ifs_val {
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
                    if !in_dq && v.is_empty() && ifs_preserves_unquoted_empty(state) {
                        push_synthetic_empty_segment(words);
                    } else {
                        push_segment(words, v, in_dq, in_dq);
                    }
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

fn ifs_preserves_unquoted_empty(state: &InterpreterState) -> bool {
    match get_var(state, "IFS") {
        Some(ifs) if !ifs.is_empty() => ifs.chars().any(|c| !matches!(c, ' ' | '\t' | '\n')),
        _ => false,
    }
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
    let extglob = state.shopt_opts.extglob;
    let mut result = Vec::new();
    let total = words.len();
    for (i, word) in words.into_iter().enumerate() {
        if i + 1 == total && word_is_synthetic_only(&word) {
            continue;
        }
        ifs_split_word(&word, &ifs, &mut result);
    }
    // When extglob is enabled, mark words containing extglob syntax as glob-eligible
    if extglob {
        for w in &mut result {
            if !w.may_glob && has_extglob_pattern(&w.text) {
                w.may_glob = true;
            }
        }
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

/// Check whether a string contains extglob syntax like `@(`, `+(`, `*(`, `?(`, `!(`.
fn has_extglob_pattern(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i + 1 < b.len() {
        if b[i] == b'\\' {
            i += 2;
            continue;
        }
        if matches!(b[i], b'@' | b'+' | b'*' | b'?' | b'!') && b[i + 1] == b'(' {
            return true;
        }
        i += 1;
    }
    false
}

/// IFS-split a single expanded word (represented as segments) into result words.
///
/// The algorithm flattens segments to character-level quotedness, then scans
/// through splitting only on unquoted IFS characters.
fn ifs_split_word(word: &[Segment], ifs: &str, result: &mut Vec<SplitWord>) {
    // Track whether any segment in the word is quoted (even if empty).
    let word_has_quoted = word.iter().any(|s| s.quoted);
    let word_has_synthetic_empty = word.iter().any(|s| s.synthetic_empty);

    // Check if the word starts/ends with an empty quoted segment (e.g. `""$A""`).
    // These anchors produce leading/trailing empty fields.
    let leading_empty_quoted = word.first().is_some_and(|s| s.quoted && s.text.is_empty());
    let trailing_empty_quoted =
        word.last().is_some_and(|s| s.quoted && s.text.is_empty()) && word.len() > 1;

    // Flatten segments to (char, quoted, glob_protected) triples.
    let chars: Vec<(char, bool, bool)> = word
        .iter()
        .flat_map(|s| s.text.chars().map(move |c| (c, s.quoted, s.glob_protected)))
        .collect();

    if chars.is_empty() {
        // An empty word with at least one quoted segment → produce one empty word.
        if word_has_quoted || word_has_synthetic_empty {
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
    let result_start = result.len();
    let mut current = String::new();
    let mut current_may_glob = false;
    let mut has_content = false;
    let mut i = 0;

    // Skip leading unquoted IFS whitespace (unless word starts with an empty
    // quoted segment like `""$A` — in that case, the leading whitespace
    // becomes a field separator after the empty anchor field).
    if leading_empty_quoted {
        // Emit the leading empty field anchor.
        result.push(SplitWord {
            text: String::new(),
            may_glob: false,
        });
    } else {
        while i < len {
            let (c, quoted, _) = chars[i];
            if !quoted && is_ifs_ws(c) {
                i += 1;
            } else {
                break;
            }
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
    let had_content = has_content || !current.is_empty();
    if had_content {
        result.push(SplitWord {
            text: current,
            may_glob: current_may_glob,
        });
    } else if word_has_quoted && result_start == result.len() && !trailing_empty_quoted {
        // All unquoted content was IFS-split away, but a quoted segment
        // (even if empty, e.g. `""`) anchors the word to produce at least
        // one empty field. Skip when trailing anchor will handle it.
        result.push(SplitWord {
            text: String::new(),
            may_glob: false,
        });
    }

    // If the word ends with an empty quoted segment (e.g. `$A""` or `""$A""`),
    // emit a trailing empty field — but only when IFS content actually
    // separated the anchor from preceding text. If the scan ended with
    // pending content (e.g. `$VAR""` with VAR="hello"), the `""` sticks
    // to the last field and does not create a separate empty field.
    if trailing_empty_quoted && !had_content {
        result.push(SplitWord {
            text: String::new(),
            may_glob: false,
        });
    }
}

fn word_is_synthetic_only(word: &[Segment]) -> bool {
    !word.is_empty() && word.iter().all(|segment| segment.synthetic_empty)
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
        extglob: state.shopt_opts.extglob,
    };

    // Parse GLOBIGNORE patterns (colon-separated list)
    let globignore_patterns: Vec<String> = get_var(state, "GLOBIGNORE")
        .filter(|s| !s.is_empty())
        .map(|s| s.split(':').map(String::from).collect())
        .unwrap_or_default();
    let has_globignore = !globignore_patterns.is_empty();

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
                let before_len = result.len();
                for p in &matches {
                    let s = p.to_string_lossy().into_owned();
                    // Apply GLOBIGNORE filtering
                    if has_globignore {
                        let basename = s.rsplit('/').next().unwrap_or(&s);
                        // When GLOBIGNORE is set, . and .. are automatically excluded
                        if basename == "." || basename == ".." {
                            continue;
                        }
                        // Match GLOBIGNORE patterns against the full path
                        if globignore_patterns
                            .iter()
                            .any(|pat| pattern::glob_match_path(pat, &s))
                        {
                            continue;
                        }
                    }
                    result.push(s);
                }
                // When GLOBIGNORE filters ALL matches, treat as no-match
                if has_globignore && result.len() == before_len {
                    if state.shopt_opts.failglob {
                        return Err(RustBashError::FailGlob {
                            pattern: w.text.clone(),
                        });
                    }
                    if state.shopt_opts.nullglob {
                        continue;
                    }
                    result.push(w.text.clone());
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
    var_name: Option<&str>,
    scalar_defined: bool,
    variable_exists: bool,
    state: &InterpreterState,
) -> String {
    match op {
        ParameterTransformOp::ToUpperCase => uppercase_matching(val, None, false),
        ParameterTransformOp::ToLowerCase => lowercase_matching(val, None, false),
        ParameterTransformOp::CapitalizeInitial => uppercase_first_matching(val, None, false),
        ParameterTransformOp::Quoted => {
            if scalar_defined {
                shell_quote(val)
            } else {
                String::new()
            }
        }
        ParameterTransformOp::ExpandEscapeSequences => expand_escape_sequences(val),
        ParameterTransformOp::PromptExpand => {
            if scalar_defined {
                expand_prompt_sequences(val, state)
            } else {
                String::new()
            }
        }
        ParameterTransformOp::PossiblyQuoteWithArraysExpanded { .. } => {
            if scalar_defined {
                shell_quote(val)
            } else {
                String::new()
            }
        }
        ParameterTransformOp::ToAssignmentLogic => {
            if variable_exists {
                var_name
                    .map(|name| format_assignment(name, state))
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
        ParameterTransformOp::ToAttributeFlags => {
            if variable_exists {
                var_name
                    .map(|name| format_attribute_flags(name, state))
                    .unwrap_or_default()
            } else {
                String::new()
            }
        }
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
                        && let Some(c) = shell_char_from_byte_escape(n)
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
                    if let Some(c) = shell_char_from_byte_escape(val_octal) {
                        result.push(c);
                    }
                }
                'c' => {
                    if i + 1 < chars.len() {
                        i += 1;
                        let ctrl = ((chars[i] as u32) & 0x1f) as u8;
                        result.push(ctrl as char);
                    } else {
                        result.push('\\');
                        result.push('c');
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

fn shell_char_from_byte_escape(value: u32) -> Option<char> {
    if value <= 0x7f {
        char::from_u32(value)
    } else if value <= 0xff {
        crate::shell_bytes::decode_shell_bytes(&[value as u8])
            .chars()
            .next()
    } else {
        char::from_u32(value)
    }
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
            let quoted = s.replace('\'', "'\\''");
            if flags == "declare -- " {
                format!("{resolved}='{quoted}'")
            } else {
                format!("{flags}{resolved}='{quoted}'")
            }
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

fn safe_upper_char(c: char) -> char {
    let mapped: Vec<char> = c.to_uppercase().collect();
    if mapped.len() == 1 { mapped[0] } else { c }
}

fn safe_lower_char(c: char) -> char {
    let mapped: Vec<char> = c.to_lowercase().collect();
    if mapped.len() == 1 { mapped[0] } else { c }
}

fn pattern_matches_char(pattern: Option<&str>, ch: char, extglob: bool) -> bool {
    pattern.is_none_or(|pat| {
        let s = ch.to_string();
        if extglob {
            pattern::extglob_match(pat, &s)
        } else {
            pattern::glob_match(pat, &s)
        }
    })
}

fn uppercase_matching(s: &str, pattern: Option<&str>, extglob: bool) -> String {
    s.chars()
        .map(|ch| {
            if pattern_matches_char(pattern, ch, extglob) {
                safe_upper_char(ch)
            } else {
                ch
            }
        })
        .collect()
}

fn lowercase_matching(s: &str, pattern: Option<&str>, extglob: bool) -> String {
    s.chars()
        .map(|ch| {
            if pattern_matches_char(pattern, ch, extglob) {
                safe_lower_char(ch)
            } else {
                ch
            }
        })
        .collect()
}

fn uppercase_first_matching(s: &str, pattern: Option<&str>, extglob: bool) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(ch) => {
            let mut result = if pattern_matches_char(pattern, ch, extglob) {
                safe_upper_char(ch).to_string()
            } else {
                ch.to_string()
            };
            result.extend(chars);
            result
        }
    }
}

fn lowercase_first_matching(s: &str, pattern: Option<&str>, extglob: bool) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(ch) => {
            let mut result = if pattern_matches_char(pattern, ch, extglob) {
                safe_lower_char(ch).to_string()
            } else {
                ch.to_string()
            };
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
    if matches!(
        parameter,
        Parameter::Special(_) | Parameter::NamedWithAllIndices { .. }
    ) {
        return Ok(());
    }
    if is_unset(state, parameter) {
        let name = parameter_name(parameter);
        return Err(RustBashError::ExpansionError {
            message: format!("{name}: unbound variable"),
            exit_code: 1,
            should_exit: true,
        });
    }
    Ok(())
}

fn negative_substring_length_error(expr: &str) -> RustBashError {
    RustBashError::ExpansionError {
        message: format!("{expr}: substring expression < 0"),
        exit_code: 1,
        should_exit: false,
    }
}

/// Reject invalid parameter names like `${%}`.
/// Bash reports "bad substitution" for these.
fn validate_parameter_name(parameter: &Parameter) -> Result<(), RustBashError> {
    if let Parameter::Named(name) = parameter
        && (name.is_empty()
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || name.starts_with(|c: char| c.is_ascii_digit()))
    {
        return Err(RustBashError::Execution(format!(
            "${{{name}}}: bad substitution"
        )));
    }
    Ok(())
}

/// Extract the parameter from any ParameterExpr variant and validate it.
fn validate_expr_parameter(expr: &ParameterExpr) -> Result<(), RustBashError> {
    let param = match expr {
        ParameterExpr::Parameter { parameter, .. }
        | ParameterExpr::UseDefaultValues { parameter, .. }
        | ParameterExpr::AssignDefaultValues { parameter, .. }
        | ParameterExpr::IndicateErrorIfNullOrUnset { parameter, .. }
        | ParameterExpr::UseAlternativeValue { parameter, .. }
        | ParameterExpr::ParameterLength { parameter, .. }
        | ParameterExpr::RemoveSmallestSuffixPattern { parameter, .. }
        | ParameterExpr::RemoveLargestSuffixPattern { parameter, .. }
        | ParameterExpr::RemoveSmallestPrefixPattern { parameter, .. }
        | ParameterExpr::RemoveLargestPrefixPattern { parameter, .. }
        | ParameterExpr::Substring { parameter, .. }
        | ParameterExpr::UppercaseFirstChar { parameter, .. }
        | ParameterExpr::UppercasePattern { parameter, .. }
        | ParameterExpr::LowercaseFirstChar { parameter, .. }
        | ParameterExpr::LowercasePattern { parameter, .. }
        | ParameterExpr::ReplaceSubstring { parameter, .. }
        | ParameterExpr::Transform { parameter, .. } => parameter,
        ParameterExpr::VariableNames { .. } | ParameterExpr::MemberKeys { .. } => return Ok(()),
    };
    validate_parameter_name(param)
}

fn validate_length_transform_syntax(word: &str) -> Result<(), RustBashError> {
    let bytes = word.as_bytes();
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        if bytes[i] == b'$' && bytes[i + 1] == b'{' && bytes[i + 2] == b'#' {
            let mut j = i + 3;
            while j < bytes.len() && bytes[j] != b'}' {
                j += 1;
            }
            if j < bytes.len() {
                let inner = &word[i + 3..j];
                let inner_bytes = inner.as_bytes();
                if let Some(end) = consume_parameter_reference_end(inner_bytes)
                    && end != inner_bytes.len()
                {
                    return Err(RustBashError::Execution(format!(
                        "${{#{inner}}}: bad substitution"
                    )));
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
    Ok(())
}

fn validate_empty_slice_syntax(word: &str) -> Result<(), RustBashError> {
    let mut i = 0usize;
    while let Some(rel_start) = word[i..].find("${") {
        let start = i + rel_start;
        let Some((body, end)) = take_parameter_body(word, start + 2) else {
            break;
        };
        if parameter_body_has_empty_slice_offset(body.as_bytes()) {
            return Err(RustBashError::Execution(format!(
                "${{{body}}}: bad substitution"
            )));
        }
        i = end + 1;
    }
    Ok(())
}

fn parameter_body_has_empty_slice_offset(body: &[u8]) -> bool {
    let Some(end) = consume_parameter_reference_end(body) else {
        return false;
    };
    end + 1 == body.len() && body.get(end) == Some(&b':')
}

fn consume_parameter_reference_end(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() {
        return None;
    }
    let mut i = 0usize;
    match bytes[i] {
        b'@' | b'*' | b'#' | b'?' | b'-' | b'$' | b'!' => i += 1,
        b'0'..=b'9' => {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        b'a'..=b'z' | b'A'..=b'Z' | b'_' => {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'[' {
                i = consume_balanced_brackets(bytes, i)?;
            }
        }
        _ => return None,
    }
    Some(i)
}

fn consume_balanced_brackets(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = start + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn validate_indirect_reference(
    expr: &ParameterExpr,
    state: &InterpreterState,
) -> Result<(), RustBashError> {
    let (parameter, indirect) = match expr {
        ParameterExpr::Parameter {
            parameter,
            indirect,
        }
        | ParameterExpr::UseDefaultValues {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::AssignDefaultValues {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::IndicateErrorIfNullOrUnset {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::UseAlternativeValue {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::ParameterLength {
            parameter,
            indirect,
        }
        | ParameterExpr::RemoveSmallestSuffixPattern {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::RemoveLargestSuffixPattern {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::RemoveSmallestPrefixPattern {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::RemoveLargestPrefixPattern {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::Substring {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::UppercaseFirstChar {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::UppercasePattern {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::LowercaseFirstChar {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::LowercasePattern {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::ReplaceSubstring {
            parameter,
            indirect,
            ..
        }
        | ParameterExpr::Transform {
            parameter,
            indirect,
            ..
        } => (parameter, *indirect),
        ParameterExpr::VariableNames { .. } | ParameterExpr::MemberKeys { .. } => {
            return Ok(());
        }
    };

    if indirect && is_unset(state, parameter) {
        return Err(RustBashError::Execution(format!(
            "{}: invalid indirect expansion",
            parameter_name(parameter)
        )));
    }

    Ok(())
}

fn resolve_parameter(parameter: &Parameter, state: &InterpreterState, indirect: bool) -> String {
    let val = resolve_parameter_direct(parameter, state);
    if indirect {
        resolve_indirect_value(&val, state)
    } else {
        val
    }
}

/// Given a string that is the value of `${!ref}`, resolve it as a variable reference.
/// Handles: simple names, `arr[idx]`, positional params (`1`, `2`), and special (`@`, `*`).
fn resolve_indirect_value(target: &str, state: &InterpreterState) -> String {
    if target.is_empty() {
        return String::new();
    }
    // Check for array subscript: name[index]
    if let Some(bracket_pos) = target.find('[')
        && target.ends_with(']')
    {
        let name = &target[..bracket_pos];
        let index_raw = &target[bracket_pos + 1..target.len() - 1];
        if index_raw == "@" || index_raw == "*" {
            // ${!ref} where ref=arr[@] or ref=arr[*]
            let concatenate = index_raw == "*";
            return resolve_all_elements(name, concatenate, state);
        }
        // Expand simple $var references in the index.
        let index = expand_simple_dollar_vars(index_raw, state);
        return resolve_array_element(name, &index, state);
    }
    // Check for positional parameter (numeric string)
    if let Ok(n) = target.parse::<u32>() {
        if n == 0 {
            return state.shell_name.clone();
        }
        return state
            .positional_params
            .get(n as usize - 1)
            .cloned()
            .unwrap_or_default();
    }
    // Check for special parameters
    match target {
        "@" => state.positional_params.join(" "),
        "*" => {
            let sep = match get_var(state, "IFS") {
                Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                None => " ".to_string(),
            };
            state.positional_params.join(&sep)
        }
        "#" => state.positional_params.len().to_string(),
        "?" => state.last_exit_code.to_string(),
        "-" => String::new(),
        "$" => "1".to_string(),
        "!" => String::new(),
        _ => get_var(state, target).unwrap_or_default(),
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

/// Strip surrounding quotes (single or double) from a string.
/// Used for associative array key lookups where `A["key"]` and `A['key']` should use `key`.
fn strip_quotes(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Resolve `${arr[index]}` — look up a specific element of an array variable.
fn resolve_array_element(name: &str, index: &str, state: &InterpreterState) -> String {
    if index.trim().is_empty() {
        return String::new();
    }
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
            let expanded_index = expand_simple_dollar_vars(index, state);
            let idx = simple_arith_eval(&expanded_index, state);
            let actual_idx = if idx < 0 {
                let max_key = map.keys().next_back().copied().unwrap_or(0);
                let resolved = max_key as i64 + 1 + idx;
                if resolved < 0 {
                    return String::new();
                }
                resolved as usize
            } else {
                idx as usize
            };
            map.get(&actual_idx).cloned().unwrap_or_default()
        }
        VariableValue::AssociativeArray(map) => {
            let key = strip_quotes(&expand_simple_dollar_vars(index, state));
            map.get(&key).cloned().unwrap_or_default()
        }
        VariableValue::Scalar(s) => {
            let expanded_index = expand_simple_dollar_vars(index, state);
            let idx = simple_arith_eval(&expanded_index, state);
            if idx == 0 || idx == -1 {
                s.clone()
            } else {
                String::new()
            }
        }
    }
}

/// Mutable variant of `resolve_array_element` that can expand `$`-references
/// and evaluate full arithmetic expressions in the index (e.g. `${a[$i]}`,
/// `${a[i-4]}`, `${a[$(echo 1)]}`).
fn resolve_array_element_mut(
    name: &str,
    index: &str,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    if index.trim().is_empty() {
        return Err(RustBashError::Execution(format!(
            "{name}: bad array subscript"
        )));
    }
    // Handle call-stack pseudo-arrays before checking env.
    if let Some(val) = resolve_call_stack_element(name, index, state) {
        return Ok(val);
    }
    use crate::interpreter::VariableValue;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);

    // Check if associative array — use key as string, not arithmetic.
    let is_assoc = state
        .env
        .get(&resolved)
        .is_some_and(|v| matches!(&v.value, VariableValue::AssociativeArray(_)));

    if is_assoc {
        // Expand $-references in the key string, then strip quotes.
        let expanded = expand_arith_expression(index, state)?;
        let key = strip_quotes(&expanded);
        let val = state
            .env
            .get(&resolved)
            .and_then(|v| {
                if let VariableValue::AssociativeArray(map) = &v.value {
                    map.get(&key).cloned()
                } else {
                    None
                }
            })
            .unwrap_or_default();
        return Ok(val);
    }

    // Indexed array or scalar: expand $-references then evaluate as arithmetic.
    let expanded = expand_arith_expression(index, state)?;
    let idx = crate::interpreter::arithmetic::eval_arithmetic(&expanded, state)?;

    let val = state
        .env
        .get(&resolved)
        .map(|var| match &var.value {
            VariableValue::IndexedArray(map) => {
                let actual_idx = if idx < 0 {
                    let max_key = map.keys().next_back().copied().unwrap_or(0);
                    let resolved_idx = max_key as i64 + 1 + idx;
                    if resolved_idx < 0 {
                        return String::new();
                    }
                    resolved_idx as usize
                } else {
                    idx as usize
                };
                map.get(&actual_idx).cloned().unwrap_or_default()
            }
            VariableValue::Scalar(s) => {
                if idx == 0 || idx == -1 {
                    s.clone()
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        })
        .unwrap_or_default();
    Ok(val)
}

/// Resolve `${FUNCNAME[i]}`, `${BASH_SOURCE[i]}`, `${BASH_LINENO[i]}` from the call stack.
/// Returns `None` if `name` is not a call-stack array, so the caller falls through to env.
fn resolve_call_stack_element(name: &str, index: &str, state: &InterpreterState) -> Option<String> {
    match name {
        "FUNCNAME" | "BASH_SOURCE" | "BASH_LINENO" => {}
        _ => return None,
    }
    let raw_idx = simple_arith_eval(index, state);
    // The call stack is ordered innermost-last; bash indexes 0 = current (innermost).
    // Build a reversed view: index 0 = top of stack, last = bottom ("main").
    let len = state.call_stack.len();
    let idx = if raw_idx < 0 {
        let resolved = len as i64 + raw_idx;
        if resolved < 0 {
            return Some(String::new());
        }
        resolved as usize
    } else {
        raw_idx as usize
    };
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
pub(crate) fn simple_arith_eval(expr: &str, state: &InterpreterState) -> i64 {
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
            if !in_dq && v.is_empty() && ifs_preserves_unquoted_empty(state) {
                push_synthetic_empty_segment(words);
            } else {
                push_segment(words, v, in_dq, in_dq);
            }
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

/// Get (key, value) pairs from an array or positional parameters.
/// Keys are numeric indices cast to `usize` for indexed arrays and positional params.
/// Used by Substring/slice expansion to support sparse-array key-based offsets.
fn get_array_kv_pairs(parameter: &Parameter, state: &InterpreterState) -> Vec<(usize, String)> {
    match parameter {
        Parameter::NamedWithAllIndices { name, .. } => {
            if let Some(vals) = get_call_stack_values(name, state) {
                return vals.into_iter().enumerate().collect();
            }
            use crate::interpreter::VariableValue;
            let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
            let Some(var) = state.env.get(&resolved) else {
                return Vec::new();
            };
            match &var.value {
                VariableValue::IndexedArray(map) => {
                    map.iter().map(|(&k, v)| (k, v.clone())).collect()
                }
                VariableValue::AssociativeArray(map) => {
                    // Assoc arrays don't have meaningful numeric keys for slicing,
                    // but bash allows it — just use enumeration order.
                    map.values()
                        .enumerate()
                        .map(|(i, v)| (i, v.clone()))
                        .collect()
                }
                VariableValue::Scalar(s) => {
                    if s.is_empty() {
                        vec![]
                    } else {
                        vec![(0, s.clone())]
                    }
                }
            }
        }
        Parameter::Special(SpecialParameter::AllPositionalParameters { .. }) => state
            .positional_params
            .iter()
            .enumerate()
            .map(|(i, v)| (i, v.clone()))
            .collect(),
        _ => Vec::new(),
    }
}

fn positional_slice_values_and_start(
    state: &InterpreterState,
    offset: i64,
) -> (Vec<String>, usize) {
    let mut values = state.positional_params.clone();
    if offset == 0 {
        values.insert(0, state.shell_name.clone());
        return (values, 0);
    }

    let start = if offset > 0 {
        (offset - 1) as usize
    } else {
        (values.len() as i64 + offset).max(0) as usize
    };

    (values, start)
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
    if state.shell_opts.allexport {
        opts.push("allexport");
    }
    // braceexpand is always on
    opts.push("braceexpand");
    if state.shell_opts.emacs_mode {
        opts.push("emacs");
    }
    if state.shell_opts.errexit {
        opts.push("errexit");
    }
    // hashall is always on
    opts.push("hashall");
    if state.shell_opts.noclobber {
        opts.push("noclobber");
    }
    if state.shell_opts.noexec {
        opts.push("noexec");
    }
    if state.shell_opts.noglob {
        opts.push("noglob");
    }
    if state.shell_opts.nounset {
        opts.push("nounset");
    }
    if state.shell_opts.pipefail {
        opts.push("pipefail");
    }
    if state.shell_opts.posix {
        opts.push("posix");
    }
    if state.shell_opts.verbose {
        opts.push("verbose");
    }
    if state.shell_opts.vi_mode {
        opts.push("vi");
    }
    if state.shell_opts.xtrace {
        opts.push("xtrace");
    }
    // Already in alphabetical order due to how we construct it
    opts.join(":")
}

/// Compute `BASHOPTS` — colon-separated list of enabled `shopt` options.
fn compute_bashopts(state: &InterpreterState) -> String {
    let o = &state.shopt_opts;
    let mut opts = Vec::new();
    // Must be alphabetical order (bash convention)
    if o.assoc_expand_once {
        opts.push("assoc_expand_once");
    }
    if o.autocd {
        opts.push("autocd");
    }
    if o.cdable_vars {
        opts.push("cdable_vars");
    }
    if o.cdspell {
        opts.push("cdspell");
    }
    if o.checkhash {
        opts.push("checkhash");
    }
    if o.checkjobs {
        opts.push("checkjobs");
    }
    if o.checkwinsize {
        opts.push("checkwinsize");
    }
    if o.cmdhist {
        opts.push("cmdhist");
    }
    if o.complete_fullquote {
        opts.push("complete_fullquote");
    }
    if o.direxpand {
        opts.push("direxpand");
    }
    if o.dirspell {
        opts.push("dirspell");
    }
    if o.dotglob {
        opts.push("dotglob");
    }
    if o.execfail {
        opts.push("execfail");
    }
    if o.expand_aliases {
        opts.push("expand_aliases");
    }
    if o.extdebug {
        opts.push("extdebug");
    }
    if o.extglob {
        opts.push("extglob");
    }
    if o.extquote {
        opts.push("extquote");
    }
    if o.failglob {
        opts.push("failglob");
    }
    if o.force_fignore {
        opts.push("force_fignore");
    }
    if o.globasciiranges {
        opts.push("globasciiranges");
    }
    if o.globskipdots {
        opts.push("globskipdots");
    }
    if o.globstar {
        opts.push("globstar");
    }
    if o.gnu_errfmt {
        opts.push("gnu_errfmt");
    }
    if o.histappend {
        opts.push("histappend");
    }
    if o.histreedit {
        opts.push("histreedit");
    }
    if o.histverify {
        opts.push("histverify");
    }
    if o.hostcomplete {
        opts.push("hostcomplete");
    }
    if o.huponexit {
        opts.push("huponexit");
    }
    if o.inherit_errexit {
        opts.push("inherit_errexit");
    }
    if o.interactive_comments {
        opts.push("interactive_comments");
    }
    if o.lastpipe {
        opts.push("lastpipe");
    }
    if o.lithist {
        opts.push("lithist");
    }
    if o.localvar_inherit {
        opts.push("localvar_inherit");
    }
    if o.localvar_unset {
        opts.push("localvar_unset");
    }
    if o.login_shell {
        opts.push("login_shell");
    }
    if o.mailwarn {
        opts.push("mailwarn");
    }
    if o.no_empty_cmd_completion {
        opts.push("no_empty_cmd_completion");
    }
    if o.nocaseglob {
        opts.push("nocaseglob");
    }
    if o.nocasematch {
        opts.push("nocasematch");
    }
    if o.nullglob {
        opts.push("nullglob");
    }
    if o.patsub_replacement {
        opts.push("patsub_replacement");
    }
    if o.progcomp {
        opts.push("progcomp");
    }
    if o.progcomp_alias {
        opts.push("progcomp_alias");
    }
    if o.promptvars {
        opts.push("promptvars");
    }
    if o.shift_verbose {
        opts.push("shift_verbose");
    }
    if o.sourcepath {
        opts.push("sourcepath");
    }
    if o.varredir_close {
        opts.push("varredir_close");
    }
    if o.xpg_echo {
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
            // Bash emits flags in canonical order: a e f h n u v x B C
            let mut flags = String::new();
            if state.shell_opts.allexport {
                flags.push('a');
            }
            if state.shell_opts.errexit {
                flags.push('e');
            }
            if state.shell_opts.noglob {
                flags.push('f');
            }
            // hashall (h) is always on in bash by default
            flags.push('h');
            if state.shell_opts.noexec {
                flags.push('n');
            }
            if state.shell_opts.nounset {
                flags.push('u');
            }
            if state.shell_opts.verbose {
                flags.push('v');
            }
            if state.shell_opts.xtrace {
                flags.push('x');
            }
            // braceexpand (B) is always on by default
            flags.push('B');
            if state.shell_opts.noclobber {
                flags.push('C');
            }
            // 's' means read from stdin — always set for non-interactive shells
            flags.push('s');
            flags
        }
    }
}

fn get_var(state: &InterpreterState, name: &str) -> Option<String> {
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    // If the resolved name is an array subscript (e.g. from a nameref to "a[2]"),
    // handle it as an array element lookup.
    if let Some(bracket_pos) = resolved.find('[')
        && resolved.ends_with(']')
    {
        let arr_name = &resolved[..bracket_pos];
        let index_raw = &resolved[bracket_pos + 1..resolved.len() - 1];
        // Expand simple $var references in the index.
        let index = expand_simple_dollar_vars(index_raw, state);
        return Some(resolve_array_element(arr_name, &index, state));
    }
    state
        .env
        .get(&resolved)
        .map(|v| v.value.as_scalar().to_string())
}

/// Expand simple `$name` variable references in a string.
/// Used for nameref targets like `A[$key]` where the index contains a variable.
fn expand_simple_dollar_vars(s: &str, state: &InterpreterState) -> String {
    if !s.contains('$') {
        return s.to_string();
    }
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            i += 1;
            let mut var_name = String::new();
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                var_name.push(chars[i]);
                i += 1;
            }
            if !var_name.is_empty() {
                let resolved_var = crate::interpreter::resolve_nameref_or_self(&var_name, state);
                let val = state
                    .env
                    .get(&resolved_var)
                    .map(|v| v.value.as_scalar().to_string())
                    .unwrap_or_default();
                result.push_str(&val);
            } else {
                result.push('$');
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn vectorized_parameter_words(
    parameter: &Parameter,
    state: &InterpreterState,
    in_dq: bool,
) -> Option<Vec<String>> {
    let (values, concatenate) = get_vectorized_values(parameter, state, false)?;
    if values.is_empty() {
        return Some(Vec::new());
    }
    if !concatenate {
        return Some(values);
    }

    let ifs_val = get_var(state, "IFS");
    let ifs_empty = matches!(&ifs_val, Some(s) if s.is_empty());
    if !in_dq && ifs_empty {
        return Some(values);
    }

    let sep = match ifs_val {
        Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
        None => " ".to_string(),
    };
    Some(vec![values.join(&sep)])
}

fn should_use_default_for_words(test_type: &ParameterTestType, words: &[String]) -> bool {
    match test_type {
        ParameterTestType::Unset => words.is_empty(),
        ParameterTestType::UnsetOrNull => {
            words.is_empty() || (words.len() == 1 && words[0].is_empty())
        }
    }
}

fn should_use_default_for_indirect_words(
    target: &Parameter,
    test_type: &ParameterTestType,
    words: &[String],
) -> bool {
    if matches!(target, Parameter::NamedWithAllIndices { .. }) {
        return words.is_empty();
    }
    should_use_default_for_words(test_type, words)
}

fn parse_indirect_target_parameter(target: &str) -> Option<Parameter> {
    if target.is_empty() {
        return None;
    }

    if let Some((name, raw_index)) = target
        .strip_suffix(']')
        .and_then(|prefix| prefix.split_once('['))
    {
        if raw_index == "@" || raw_index == "*" {
            return Some(Parameter::NamedWithAllIndices {
                name: name.to_string(),
                concatenate: raw_index == "*",
            });
        }
        return Some(Parameter::NamedWithIndex {
            name: name.to_string(),
            index: raw_index.to_string(),
        });
    }

    if let Ok(n) = target.parse::<u32>() {
        return Some(Parameter::Positional(n));
    }

    match target {
        "@" => Some(Parameter::Special(
            SpecialParameter::AllPositionalParameters { concatenate: false },
        )),
        "*" => Some(Parameter::Special(
            SpecialParameter::AllPositionalParameters { concatenate: true },
        )),
        "#" => Some(Parameter::Special(
            SpecialParameter::PositionalParameterCount,
        )),
        "?" => Some(Parameter::Special(SpecialParameter::LastExitStatus)),
        "-" => Some(Parameter::Special(SpecialParameter::CurrentOptionFlags)),
        "$" => Some(Parameter::Special(SpecialParameter::ProcessId)),
        "!" => Some(Parameter::Special(
            SpecialParameter::LastBackgroundProcessId,
        )),
        "0" => Some(Parameter::Special(SpecialParameter::ShellName)),
        _ => Some(Parameter::Named(target.to_string())),
    }
}

fn should_use_default_for_parameter(
    parameter: &Parameter,
    indirect: bool,
    val: &str,
    test_type: &ParameterTestType,
    state: &InterpreterState,
    in_dq: bool,
) -> bool {
    if indirect {
        let target_name = resolve_parameter(parameter, state, false);
        if let Some(target_param) = parse_indirect_target_parameter(&target_name) {
            if let Some(words) = vectorized_parameter_words(&target_param, state, in_dq) {
                return should_use_default_for_indirect_words(&target_param, test_type, &words);
            }
            return should_use_default(val, test_type, state, &target_param);
        }
        return true;
    }

    if let Some(words) = vectorized_parameter_words(parameter, state, in_dq) {
        should_use_default_for_words(test_type, &words)
    } else {
        should_use_default(val, test_type, state, parameter)
    }
}

fn should_use_default_for_parameter_mut(
    parameter: &Parameter,
    indirect: bool,
    val: &str,
    test_type: &ParameterTestType,
    state: &mut InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    if indirect {
        let target_name = resolve_parameter_maybe_mut(parameter, state, false)?;
        if let Some(target_param) = parse_indirect_target_parameter(&target_name) {
            if let Some(words) = vectorized_parameter_words(&target_param, state, in_dq) {
                return Ok(should_use_default_for_indirect_words(
                    &target_param,
                    test_type,
                    &words,
                ));
            }
            return Ok(should_use_default(val, test_type, state, &target_param));
        }
        return Ok(true);
    }

    if let Some(words) = vectorized_parameter_words(parameter, state, in_dq) {
        Ok(should_use_default_for_words(test_type, &words))
    } else {
        Ok(should_use_default(val, test_type, state, parameter))
    }
}

fn push_expanded_parameter_value(
    parameter: &Parameter,
    indirect: bool,
    val: &str,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
) {
    if !indirect && let Some((values, concatenate)) = get_vectorized_values(parameter, state, false)
    {
        push_vectorized(values, concatenate, words, state, in_dq);
    } else {
        push_segment(words, val, in_dq, in_dq);
    }
}

fn resolve_array_assignment_index(
    name: &str,
    index_expr: &str,
    state: &mut InterpreterState,
) -> Result<usize, RustBashError> {
    let expanded = expand_arith_expression(index_expr, state)?;
    let idx = crate::interpreter::arithmetic::eval_arithmetic(&expanded, state)?;
    if idx >= 0 {
        return Ok(idx as usize);
    }

    let resolved_name = crate::interpreter::resolve_nameref_or_self(name, state);
    let max_key = state.env.get(&resolved_name).and_then(|v| match &v.value {
        crate::interpreter::VariableValue::IndexedArray(map) => map.keys().next_back().copied(),
        crate::interpreter::VariableValue::Scalar(_) => Some(0),
        _ => None,
    });

    match max_key {
        Some(mk) => {
            let resolved = mk as i64 + 1 + idx;
            if resolved < 0 {
                Err(RustBashError::Execution(format!(
                    "{name}: bad array subscript"
                )))
            } else {
                Ok(resolved as usize)
            }
        }
        None => Err(RustBashError::Execution(format!(
            "{name}: bad array subscript"
        ))),
    }
}

fn assign_default_to_parameter(
    parameter: &Parameter,
    indirect: bool,
    value: &str,
    state: &mut InterpreterState,
) -> Result<(), RustBashError> {
    if indirect {
        let target_name = resolve_parameter_maybe_mut(parameter, state, false)?;
        if !target_name.is_empty() {
            set_variable(state, &target_name, value.to_string())?;
        }
        return Ok(());
    }

    match parameter {
        Parameter::Named(name) => set_variable(state, name, value.to_string())?,
        Parameter::NamedWithIndex { name, index } => {
            let resolved_name = crate::interpreter::resolve_nameref_or_self(name, state);
            let is_assoc = state.env.get(&resolved_name).is_some_and(|var| {
                matches!(
                    var.value,
                    crate::interpreter::VariableValue::AssociativeArray(_)
                )
            });
            if is_assoc {
                let key = strip_quotes(&expand_arith_expression(index, state)?);
                set_assoc_element(state, &resolved_name, key, value.to_string())?;
            } else {
                let idx = resolve_array_assignment_index(&resolved_name, index, state)?;
                crate::interpreter::set_array_element(
                    state,
                    &resolved_name,
                    idx,
                    value.to_string(),
                )?;
            }
        }
        _ => {}
    }
    Ok(())
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
            match state.env.get(&resolved) {
                None => true,
                Some(var) => {
                    // Variables with DECLARED_ONLY (e.g. `local x`) are unset
                    if var
                        .attrs
                        .contains(crate::interpreter::VariableAttrs::DECLARED_ONLY)
                    {
                        return true;
                    }
                    // For indexed arrays, $name is equivalent to ${name[0]},
                    // so it's "unset" if index 0 is not present.
                    use crate::interpreter::VariableValue;
                    match &var.value {
                        VariableValue::IndexedArray(map) => !map.contains_key(&0),
                        VariableValue::AssociativeArray(_) => false,
                        _ => false,
                    }
                }
            }
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
                            let expanded_index = expand_simple_dollar_vars(index, state);
                            let idx = simple_arith_eval(&expanded_index, state);
                            let actual_idx = if idx < 0 {
                                let max_key = map.keys().next_back().copied().unwrap_or(0);
                                let resolved_idx = max_key as i64 + 1 + idx;
                                if resolved_idx < 0 {
                                    return true;
                                }
                                resolved_idx as usize
                            } else {
                                idx as usize
                            };
                            !map.contains_key(&actual_idx)
                        }
                        VariableValue::AssociativeArray(map) => {
                            let key = expand_simple_dollar_vars(index, state);
                            !map.contains_key(key.as_str())
                        }
                        VariableValue::Scalar(_) => {
                            let expanded_index = expand_simple_dollar_vars(index, state);
                            let idx = simple_arith_eval(&expanded_index, state);
                            idx != 0 && idx != -1
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

fn parameter_variable_exists(
    parameter: &Parameter,
    indirect: bool,
    state: &InterpreterState,
) -> bool {
    if indirect {
        let target = resolve_parameter(parameter, state, false);
        if let Some(target_param) = parse_indirect_target_parameter(&target) {
            return parameter_variable_exists(&target_param, false, state);
        }
        return false;
    }

    match parameter {
        Parameter::Named(name)
        | Parameter::NamedWithIndex { name, .. }
        | Parameter::NamedWithAllIndices { name, .. } => {
            let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
            state.env.contains_key(&resolved)
        }
        Parameter::Positional(n) => {
            if *n == 0 {
                true
            } else {
                state.positional_params.get(*n as usize - 1).is_some()
            }
        }
        Parameter::Special(_) => true,
    }
}

fn parameter_is_associative_array(
    parameter: &Parameter,
    indirect: bool,
    state: &InterpreterState,
) -> bool {
    if indirect {
        let target = resolve_parameter(parameter, state, false);
        if let Some(target_param) = parse_indirect_target_parameter(&target) {
            return parameter_is_associative_array(&target_param, false, state);
        }
        return false;
    }

    match parameter {
        Parameter::Named(name)
        | Parameter::NamedWithIndex { name, .. }
        | Parameter::NamedWithAllIndices { name, .. } => {
            let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
            state.env.get(&resolved).is_some_and(|var| {
                matches!(
                    var.value,
                    crate::interpreter::VariableValue::AssociativeArray(_)
                )
            })
        }
        _ => false,
    }
}

fn parameter_scalar_is_unset(
    parameter: &Parameter,
    indirect: bool,
    state: &InterpreterState,
) -> bool {
    if indirect {
        let target = resolve_parameter(parameter, state, false);
        if let Some(target_param) = parse_indirect_target_parameter(&target) {
            return parameter_scalar_is_unset(&target_param, false, state);
        }
        return true;
    }
    if let Parameter::Named(name) = parameter {
        if is_dynamic_special(name) {
            return false;
        }
        let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
        if let Some(var) = state.env.get(&resolved) {
            if var
                .attrs
                .contains(crate::interpreter::VariableAttrs::DECLARED_ONLY)
            {
                return true;
            }
            if let crate::interpreter::VariableValue::AssociativeArray(map) = &var.value {
                return !map.contains_key("0");
            }
        }
    }
    is_unset(state, parameter)
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

fn transform_target_name(
    parameter: &Parameter,
    indirect: bool,
    state: &InterpreterState,
) -> Option<String> {
    if indirect {
        let target = resolve_parameter(parameter, state, false);
        return transform_target_name_from_str(&target);
    }

    match parameter {
        Parameter::Named(name)
        | Parameter::NamedWithIndex { name, .. }
        | Parameter::NamedWithAllIndices { name, .. } => Some(name.clone()),
        _ => None,
    }
}

fn transform_target_name_from_str(target: &str) -> Option<String> {
    if target.is_empty() {
        return None;
    }
    if let Some((name, _)) = target
        .strip_suffix(']')
        .and_then(|prefix| prefix.split_once('['))
    {
        return Some(name.to_string());
    }
    if target
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !target.starts_with(|c: char| c.is_ascii_digit())
    {
        Some(target.to_string())
    } else {
        None
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
        expand_raw_piece(&piece_ws.piece, &mut words, state, in_dq)?;
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
        expand_raw_piece_mut(&piece_ws.piece, &mut words, state, in_dq)?;
    }
    let result = finalize_no_split(words);
    Ok(result.join(" "))
}

/// Expand a word piece from a parameter expansion operand.
/// When `in_dq` is true, single quotes are literal characters (not quote
/// delimiters), matching bash behavior for e.g. `"${var:-'hello'}"`.
fn expand_raw_piece(
    piece: &WordPiece,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    if in_dq && let WordPiece::SingleQuotedText(s) = piece {
        // Inside DQ context, single quotes are literal characters.
        push_segment(words, &format!("'{s}'"), true, true);
        return Ok(false);
    }
    expand_word_piece(piece, words, state, in_dq)
}

/// Mutable variant of `expand_raw_piece`.
fn expand_raw_piece_mut(
    piece: &WordPiece,
    words: &mut Vec<WordInProgress>,
    state: &mut InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    if in_dq && let WordPiece::SingleQuotedText(s) = piece {
        push_segment(words, &format!("'{s}'"), true, true);
        return Ok(false);
    }
    expand_word_piece_mut(piece, words, state, in_dq)
}

/// Expand a parameter expansion operand (default value, alternative value)
/// directly into the word list, preserving inner quoting for proper IFS splitting.
///
/// Unlike `expand_raw_string_ctx` which collapses to a single string, this
/// preserves the quoting structure of inner quoted segments so that IFS splitting
/// correctly separates words: `${Unset:-"a b" c}` → `["a b", "c"]`.
fn expand_raw_into_words(
    raw: &str,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
) -> Result<(), RustBashError> {
    let options = parser_options();
    let pieces = brush_parser::word::parse(raw, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;
    for piece_ws in &pieces {
        expand_default_piece(&piece_ws.piece, words, state, in_dq)?;
    }
    Ok(())
}

fn expand_raw_into_words_mut(
    raw: &str,
    words: &mut Vec<WordInProgress>,
    state: &mut InterpreterState,
    in_dq: bool,
) -> Result<(), RustBashError> {
    let options = parser_options();
    let pieces = brush_parser::word::parse(raw, &options)
        .map_err(|e| RustBashError::Parse(e.to_string()))?;
    for piece_ws in &pieces {
        expand_default_piece_mut(&piece_ws.piece, words, state, in_dq)?;
    }
    Ok(())
}

/// Expand a word piece in default/alternative value context.
///
/// When not in double-quote context, literal `Text` pieces are pushed as
/// unquoted (subject to IFS splitting), matching bash behavior where
/// `${Unset:-a b}` word-splits like a bare `a b`.
///
/// When in DQ context, single-quoted text has its single quotes treated as
/// literal characters with the content undergoing parameter expansion,
/// matching bash behavior where `"${Unset:-'$var'}"` expands `$var`.
fn expand_default_piece(
    piece: &WordPiece,
    words: &mut Vec<WordInProgress>,
    state: &InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    if in_dq {
        if let WordPiece::SingleQuotedText(s) = piece {
            // Inside DQ, single quotes are literal characters.
            // The content undergoes parameter expansion.
            push_segment(words, "'", true, true);
            let options = parser_options();
            if let Ok(inner_pieces) = brush_parser::word::parse(s, &options) {
                for inner in &inner_pieces {
                    expand_word_piece(&inner.piece, words, state, true)?;
                }
            } else {
                push_segment(words, s, true, true);
            }
            push_segment(words, "'", true, true);
            return Ok(false);
        }
        // Inside parameter expansion, \} was used to escape the closing brace.
        // After parsing, quote removal should strip the backslash.
        if let WordPiece::EscapeSequence(s) = piece
            && let Some(c) = s.strip_prefix('\\')
            && c == "}"
        {
            push_segment(words, c, true, true);
            return Ok(false);
        }
        return expand_word_piece(piece, words, state, true);
    }
    // Outside DQ: literal text is unquoted (subject to IFS splitting)
    if let WordPiece::Text(s) = piece {
        push_segment(words, s, false, false);
        return Ok(false);
    }
    expand_word_piece(piece, words, state, false)
}

fn expand_default_piece_mut(
    piece: &WordPiece,
    words: &mut Vec<WordInProgress>,
    state: &mut InterpreterState,
    in_dq: bool,
) -> Result<bool, RustBashError> {
    if in_dq {
        if let WordPiece::SingleQuotedText(s) = piece {
            push_segment(words, "'", true, true);
            let options = parser_options();
            if let Ok(inner_pieces) = brush_parser::word::parse(s, &options) {
                for inner in &inner_pieces {
                    expand_word_piece_mut(&inner.piece, words, state, true)?;
                }
            } else {
                push_segment(words, s, true, true);
            }
            push_segment(words, "'", true, true);
            return Ok(false);
        }
        if let WordPiece::EscapeSequence(s) = piece
            && let Some(c) = s.strip_prefix('\\')
            && c == "}"
        {
            push_segment(words, c, true, true);
            return Ok(false);
        }
        return expand_word_piece_mut(piece, words, state, true);
    }
    if let WordPiece::Text(s) = piece {
        push_segment(words, s, false, false);
        return Ok(false);
    }
    expand_word_piece_mut(piece, words, state, false)
}

/// Expand a pattern string from a strip/replace operator, processing quotes.
///
/// Single quotes, double quotes, and ANSI-C quotes within patterns are
/// always respected as quoting delimiters (even inside double-quoted
/// `"${var%'pattern'}"`), and quote removal is performed on the result.
/// Characters from inside quotes that are pattern-special (`?`, `*`, `[`, `]`)
/// are backslash-escaped so the pattern matcher treats them as literal.
/// Backslash escapes outside quotes are preserved for the pattern matcher.
fn expand_pattern_string(pat: &str, state: &InterpreterState) -> Result<String, RustBashError> {
    let mut result = String::new();
    let mut chars = pat.chars().peekable();
    let mut at_word_start = true;

    while let Some(c) = chars.next() {
        match c {
            '~' if at_word_start => {
                let home = get_var(state, "HOME").unwrap_or_default();
                if home.is_empty() {
                    result.push('~');
                } else {
                    result.push_str(&home);
                }
                at_word_start = false;
            }
            '\\' => {
                if let Some(&next) = chars.peek() {
                    chars.next();
                    if matches!(
                        next,
                        '?' | '*' | '[' | ']' | '\\' | '(' | ')' | '|' | '!' | '+' | '@'
                    ) {
                        result.push('\\');
                    }
                    result.push(next);
                } else {
                    result.push('\\');
                }
                at_word_start = false;
            }
            '\'' => {
                // Single quote: take content literally, escape glob chars
                for ch in chars.by_ref() {
                    if ch == '\'' {
                        break;
                    }
                    if matches!(ch, '?' | '*' | '[' | ']' | '\\') {
                        result.push('\\');
                    }
                    result.push(ch);
                }
                at_word_start = false;
            }
            '$' if chars.peek() == Some(&'\'') => {
                // ANSI-C quote $'...' — escape glob chars in result
                chars.next(); // skip opening '
                while let Some(ch) = chars.next() {
                    if ch == '\'' {
                        break;
                    }
                    if ch == '\\' {
                        if let Some(esc) = chars.next() {
                            let decoded = match esc {
                                'n' => '\n',
                                't' => '\t',
                                'r' => '\r',
                                '\\' => '\\',
                                '\'' => '\'',
                                'a' => '\x07',
                                'b' => '\x08',
                                'e' | 'E' => '\x1b',
                                'f' => '\x0c',
                                'v' => '\x0b',
                                _ => {
                                    // Unknown escape — push both chars.
                                    // Backslash is glob-special, so always escape it.
                                    result.push('\\');
                                    result.push('\\');
                                    if matches!(esc, '?' | '*' | '[' | ']' | '\\') {
                                        result.push('\\');
                                    }
                                    result.push(esc);
                                    continue;
                                }
                            };
                            if matches!(decoded, '?' | '*' | '[' | ']' | '\\') {
                                result.push('\\');
                            }
                            result.push(decoded);
                        }
                    } else {
                        if matches!(ch, '?' | '*' | '[' | ']' | '\\') {
                            result.push('\\');
                        }
                        result.push(ch);
                    }
                }
                at_word_start = false;
            }
            '"' => {
                // Double quote: expand variables inside, remove quote delimiters.
                // Escape glob chars in expanded content.
                let mut inner = String::new();
                while let Some(ch) = chars.next() {
                    if ch == '"' {
                        break;
                    }
                    if ch == '\\' {
                        if let Some(&next) = chars.peek()
                            && matches!(next, '$' | '`' | '"' | '\\')
                        {
                            inner.push(next);
                            chars.next();
                            continue;
                        }
                        inner.push('\\');
                    } else {
                        inner.push(ch);
                    }
                }
                let expanded = expand_raw_string_ctx(&inner, state, true)?;
                for ch in expanded.chars() {
                    if matches!(ch, '?' | '*' | '[' | ']' | '\\') {
                        result.push('\\');
                    }
                    result.push(ch);
                }
                at_word_start = false;
            }
            '$' => {
                if let Some(expanded) = expand_simple_parameter_reference(&mut chars, state) {
                    result.push_str(&expanded);
                } else {
                    result.push('$');
                }
                at_word_start = false;
            }
            _ => {
                result.push(c);
                at_word_start = false;
            }
        }
    }
    Ok(result)
}

/// Expand a replacement string from a `${var//pattern/replacement}` operator.
///
/// Quote removal is performed but glob characters are NOT escaped,
/// since the replacement is not used as a pattern.
fn expand_replacement_string(
    repl: &str,
    state: &InterpreterState,
) -> Result<String, RustBashError> {
    let mut result = String::new();
    let mut chars = repl.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                if let Some(&next) = chars.peek()
                    && matches!(next, '/' | '\\')
                {
                    result.push(next);
                    chars.next();
                } else {
                    result.push('\\');
                }
            }
            '\'' => {
                for ch in chars.by_ref() {
                    if ch == '\'' {
                        break;
                    }
                    result.push(ch);
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                while let Some(ch) = chars.next() {
                    if ch == '\'' {
                        break;
                    }
                    if ch == '\\' {
                        if let Some(esc) = chars.next() {
                            match esc {
                                'n' => result.push('\n'),
                                't' => result.push('\t'),
                                'r' => result.push('\r'),
                                '\\' => result.push('\\'),
                                '\'' => result.push('\''),
                                'a' => result.push('\x07'),
                                'b' => result.push('\x08'),
                                'e' | 'E' => result.push('\x1b'),
                                'f' => result.push('\x0c'),
                                'v' => result.push('\x0b'),
                                _ => {
                                    result.push('\\');
                                    result.push(esc);
                                }
                            }
                        }
                    } else {
                        result.push(ch);
                    }
                }
            }
            '"' => {
                let mut inner = String::new();
                while let Some(ch) = chars.next() {
                    if ch == '"' {
                        break;
                    }
                    if ch == '\\' {
                        if let Some(&next) = chars.peek()
                            && matches!(next, '$' | '`' | '"' | '\\')
                        {
                            inner.push(next);
                            chars.next();
                            continue;
                        }
                        inner.push('\\');
                    } else {
                        inner.push(ch);
                    }
                }
                let expanded = expand_raw_string_ctx(&inner, state, true)?;
                result.push_str(&expanded);
            }
            '$' => {
                if let Some(expanded) = expand_simple_parameter_reference(&mut chars, state) {
                    result.push_str(&expanded);
                } else {
                    result.push('$');
                }
            }
            _ => {
                result.push(c);
            }
        }
    }
    Ok(result)
}

fn expand_simple_parameter_reference(
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
    state: &InterpreterState,
) -> Option<String> {
    if chars.peek() == Some(&'{') {
        chars.next();
        let mut name = String::new();
        for ch in chars.by_ref() {
            if ch == '}' {
                break;
            }
            name.push(ch);
        }
        return Some(resolve_pattern_var(&name, state));
    }

    if let Some(&ch) = chars.peek()
        && matches!(ch, '@' | '*' | '#' | '?' | '-' | '$' | '!')
    {
        chars.next();
        return Some(resolve_pattern_var(&ch.to_string(), state));
    }

    let mut name = String::new();
    while let Some(&ch) = chars.peek() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            chars.next();
            name.push(ch);
        } else {
            break;
        }
    }

    if name.is_empty() {
        None
    } else {
        Some(resolve_pattern_var(&name, state))
    }
}

fn resolve_pattern_var(name: &str, state: &InterpreterState) -> String {
    if name.chars().all(|ch| ch.is_ascii_digit()) {
        return if name == "0" {
            state.shell_name.clone()
        } else {
            name.parse::<usize>()
                .ok()
                .and_then(|n| state.positional_params.get(n.saturating_sub(1)))
                .cloned()
                .unwrap_or_default()
        };
    }

    match name {
        "@" => return state.positional_params.join(" "),
        "*" => {
            let sep = match get_var(state, "IFS") {
                Some(s) => s.chars().next().map(|c| c.to_string()).unwrap_or_default(),
                None => " ".to_string(),
            };
            return state.positional_params.join(&sep);
        }
        "#" => return state.positional_params.len().to_string(),
        "?" => return state.last_exit_code.to_string(),
        "-" => return String::new(),
        "$" => return "1".to_string(),
        "!" => return String::new(),
        _ => {}
    }

    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    state
        .env
        .get(&resolved)
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_default()
}

fn normalize_patsub_slashes<'a>(
    pattern: &'a str,
    replacement: Option<&'a str>,
) -> (&'a str, Option<&'a str>) {
    if pattern.is_empty()
        && let Some(repl) = replacement
        && let Some(stripped) = repl.strip_prefix('/')
    {
        return ("/", Some(stripped));
    }
    (pattern, replacement)
}

fn is_byte_locale(state: &InterpreterState) -> bool {
    matches!(
        get_var(state, "LC_ALL").as_deref(),
        Some("C") | Some("POSIX")
    )
}

fn string_length(val: &str, state: &InterpreterState) -> usize {
    if is_byte_locale(state) {
        val.len()
    } else {
        val.chars().count()
    }
}

/// This handles cases like `$((${zero}11))` where `zero=0` should yield `011`.
pub(crate) fn expand_arith_expression(
    expr: &str,
    state: &mut InterpreterState,
) -> Result<String, RustBashError> {
    // Preserve literal quotes and # characters when no shell expansion is needed so
    // the arithmetic tokenizer can reject them with bash-like errors.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::{
        ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, ShoptOpts, Variable,
        VariableAttrs, VariableValue,
    };
    use crate::network::NetworkPolicy;
    use crate::vfs::InMemoryFs;
    use brush_parser::word::{ParameterExpr, WordPiece};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Arc;

    fn make_state() -> InterpreterState {
        InterpreterState {
            fs: Arc::new(InMemoryFs::new()),
            env: HashMap::from([(
                "foo".to_string(),
                Variable {
                    value: VariableValue::Scalar("a b c d".to_string()),
                    attrs: VariableAttrs::empty(),
                },
            )]),
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
            shell_start_time: crate::platform::Instant::now(),
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

    #[test]
    fn parser_keeps_double_spaces_in_strip_pattern() {
        let pieces =
            brush_parser::word::parse("${foo%c  d}", &crate::interpreter::parser_options())
                .unwrap();
        let pattern = match &pieces[0].piece {
            WordPiece::ParameterExpansion(ParameterExpr::RemoveSmallestSuffixPattern {
                pattern: Some(pattern),
                ..
            }) => pattern,
            other => panic!("unexpected parse result: {other:?}"),
        };
        assert_eq!(pattern, "c  d");
    }

    #[test]
    fn strip_pattern_respects_double_space_literals() {
        let mut state = make_state();
        let first = ast::Word {
            value: "\"${foo%c d}\"".to_string(),
            loc: None,
        };
        let second = ast::Word {
            value: "\"${foo%c  d}\"".to_string(),
            loc: None,
        };

        let first = expand_word_mut(&first, &mut state).unwrap();
        let second = expand_word_mut(&second, &mut state).unwrap();

        assert_eq!(first, vec!["a b ".to_string()]);
        assert_eq!(second, vec!["a b c d".to_string()]);
    }

    #[test]
    fn command_parser_keeps_double_spaces_in_quoted_strip_pattern() {
        let program = crate::interpreter::parse("argv.py \"${foo%c d}\" \"${foo%c  d}\"").unwrap();
        let pipeline = &program.complete_commands[0].0[0].0.first;
        let cmd = match &pipeline.seq[0] {
            ast::Command::Simple(simple) => simple,
            other => panic!("unexpected command: {other:?}"),
        };

        let suffix = cmd.suffix.as_ref().unwrap();
        let second = match &suffix.0[0] {
            ast::CommandPrefixOrSuffixItem::Word(word) => word,
            other => panic!("unexpected suffix item: {other:?}"),
        };
        assert_eq!(second.value, "\"${foo%c d}\"");

        let third = match &suffix.0[1] {
            ast::CommandPrefixOrSuffixItem::Word(word) => word,
            other => panic!("unexpected suffix item: {other:?}"),
        };
        assert_eq!(third.value, "\"${foo%c  d}\"");
    }

    #[test]
    fn length_slice_syntax_is_bad_substitution() {
        let mut state = make_state();
        let word = ast::Word {
            value: "${#foo:1:3}".to_string(),
            loc: None,
        };
        let err = expand_word_mut(&word, &mut state).unwrap_err();
        assert!(matches!(
            err,
            RustBashError::Execution(msg) if msg.contains("bad substitution")
        ));
    }

    #[test]
    fn empty_slice_offset_is_bad_substitution() {
        let mut state = make_state();
        let word = ast::Word {
            value: "${foo:}".to_string(),
            loc: None,
        };
        let err = expand_word_mut(&word, &mut state).unwrap_err();
        assert!(matches!(
            err,
            RustBashError::Execution(msg) if msg.contains("bad substitution")
        ));
    }

    #[test]
    fn slice_respects_nounset() {
        let mut state = make_state();
        state.shell_opts.nounset = true;
        let word = ast::Word {
            value: "${undef:1:2}".to_string(),
            loc: None,
        };
        let err = expand_word_mut(&word, &mut state).unwrap_err();
        assert!(matches!(
            err,
            RustBashError::ExpansionError { message, .. }
                if message.contains("undef: unbound variable")
        ));
    }

    #[test]
    fn positional_slice_zero_offset_includes_shell_name() {
        let mut state = make_state();
        state.shell_name = "shell".to_string();
        state.positional_params = vec!["a 1".to_string(), "b 2".to_string()];
        let word = ast::Word {
            value: "\"${@:0:2}\"".to_string(),
            loc: None,
        };
        assert_eq!(
            expand_word_mut(&word, &mut state).unwrap(),
            vec!["shell".to_string(), "a 1".to_string()]
        );
    }

    #[test]
    fn immutable_positional_slice_negative_length_is_an_error() {
        let mut state = make_state();
        state.positional_params = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let word = ast::Word {
            value: "\"${@:2:-3}\"".to_string(),
            loc: None,
        };
        let err = expand_word(&word, &state).unwrap_err();
        assert!(matches!(
            err,
            RustBashError::ExpansionError { message, .. }
                if message.contains("-3: substring expression < 0")
        ));
    }

    #[test]
    fn mutable_array_slice_negative_length_reports_length_expr() {
        let mut state = make_state();
        state.env.insert(
            "arr".to_string(),
            Variable {
                value: VariableValue::IndexedArray(BTreeMap::from([
                    (0, "a".to_string()),
                    (1, "b".to_string()),
                    (2, "c".to_string()),
                ])),
                attrs: VariableAttrs::empty(),
            },
        );
        let word = ast::Word {
            value: "\"${arr[@]:1:-2}\"".to_string(),
            loc: None,
        };
        let err = expand_word_mut(&word, &mut state).unwrap_err();
        assert!(matches!(
            err,
            RustBashError::ExpansionError { message, .. }
                if message.contains("-2: substring expression < 0")
        ));
    }

    #[test]
    fn brace_expansion_precedes_tilde_for_root_home_mix() {
        let mut state = make_state();
        state.env.insert(
            "HOME".to_string(),
            Variable {
                value: VariableValue::Scalar("/home/bob".to_string()),
                attrs: VariableAttrs::empty(),
            },
        );
        let word = ast::Word {
            value: "~{/src,root}".to_string(),
            loc: None,
        };
        assert_eq!(
            expand_word_mut(&word, &mut state).unwrap(),
            vec!["/home/bob/src".to_string(), "/root".to_string()]
        );
    }
}
