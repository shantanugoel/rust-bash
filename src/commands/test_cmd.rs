//! Implementation of the `test` and `[` shell commands.
//!
//! Evaluates conditional expressions for file tests, string comparisons,
//! numeric comparisons, and logical operators. Returns exit code 0 for
//! true, 1 for false, and 2 for usage errors.

use crate::commands::{CommandContext, CommandResult};
use crate::interpreter::pattern::glob_match;
use crate::vfs::{NodeType, VirtualFs};
use std::path::{Path, PathBuf};

/// Evaluate a `test` / `[` expression from a list of string arguments.
/// Returns exit code: 0 = true, 1 = false, 2 = error.
pub(crate) fn evaluate_test_args(args: &[String], ctx: &CommandContext) -> CommandResult {
    if args.is_empty() {
        // `test` with no args is false
        return result(1);
    }

    match eval_expr(args, ctx) {
        Ok((value, consumed)) => {
            if consumed != args.len() {
                error_result("too many arguments")
            } else {
                result(if value { 0 } else { 1 })
            }
        }
        Err(msg) => error_result(&msg),
    }
}

/// Recursive-descent parser for test expressions.
/// Returns (bool result, number of tokens consumed).
fn eval_expr(args: &[String], ctx: &CommandContext) -> Result<(bool, usize), String> {
    eval_or(args, ctx)
}

/// Parse: expr_or := expr_and ( '-o' expr_and )*
fn eval_or(args: &[String], ctx: &CommandContext) -> Result<(bool, usize), String> {
    let (mut val, mut pos) = eval_and(args, ctx)?;
    while pos < args.len() && args[pos] == "-o" {
        let (right, consumed) = eval_and(&args[pos + 1..], ctx)?;
        val = val || right;
        pos += 1 + consumed;
    }
    Ok((val, pos))
}

/// Parse: expr_and := expr_not ( '-a' expr_not )*
fn eval_and(args: &[String], ctx: &CommandContext) -> Result<(bool, usize), String> {
    let (mut val, mut pos) = eval_not(args, ctx)?;
    while pos < args.len() && args[pos] == "-a" {
        let (right, consumed) = eval_not(&args[pos + 1..], ctx)?;
        val = val && right;
        pos += 1 + consumed;
    }
    Ok((val, pos))
}

/// Parse: expr_not := '!' expr_not | primary
fn eval_not(args: &[String], ctx: &CommandContext) -> Result<(bool, usize), String> {
    if args.is_empty() {
        return Err("argument expected".to_string());
    }
    if args[0] == "!" {
        if args.len() < 2 {
            // Single "!" → non-empty string → true (POSIX 1-arg rule)
            return Ok((true, 1));
        }
        let (val, consumed) = eval_not(&args[1..], ctx)?;
        Ok((!val, 1 + consumed))
    } else {
        eval_primary(args, ctx)
    }
}

/// Parse a primary test expression.
fn eval_primary(args: &[String], ctx: &CommandContext) -> Result<(bool, usize), String> {
    if args.is_empty() {
        return Err("argument expected".to_string());
    }

    // Parenthesized expression: ( expr )
    if args[0] == "(" {
        let (val, consumed) = eval_expr(&args[1..], ctx)?;
        if 1 + consumed >= args.len() || args[1 + consumed] != ")" {
            return Err("missing ')'".to_string());
        }
        return Ok((val, 2 + consumed)); // ( + consumed + )
    }

    // Try binary operators (3-token): operand OP operand
    if args.len() >= 3
        && let Some(val) = try_binary(&args[0], &args[1], &args[2], ctx)
    {
        return Ok((val, 3));
    }

    // Unary operators (2-token): OP operand
    if args.len() >= 2
        && let Some(val) = try_unary(&args[0], &args[1], ctx)
    {
        return Ok((val, 2));
    }

    // Single argument: true if non-empty string
    Ok((!args[0].is_empty(), 1))
}

/// Try to evaluate a unary test. Returns None if `op` isn't a unary operator.
fn try_unary(op: &str, operand: &str, ctx: &CommandContext) -> Option<bool> {
    match op {
        // String tests
        "-z" => Some(operand.is_empty()),
        "-n" => Some(!operand.is_empty()),

        // File tests
        "-a" | "-e" => Some(file_exists(operand, ctx)),
        "-f" => Some(file_is_regular(operand, ctx)),
        "-d" => Some(file_is_dir(operand, ctx)),
        "-L" | "-h" => Some(file_is_symlink(operand, ctx)),
        "-s" => Some(file_size_nonzero(operand, ctx)),
        "-r" => Some(file_exists(operand, ctx)), // always readable in VFS
        "-w" => Some(file_exists(operand, ctx)), // always writable in VFS
        "-x" => Some(file_exists(operand, ctx)), // always executable in VFS
        "-O" | "-G" => Some(file_exists(operand, ctx)), // always owned by current user in VFS
        "-b" | "-c" | "-p" | "-S" | "-u" | "-g" | "-k" | "-t" | "-N" => {
            Some(false) // unsupported file tests always false
        }
        "-o" => {
            // Check if shell option is enabled
            Some(is_shell_option_set(operand, ctx))
        }
        "-v" => {
            // Shell variable is set — check array elements if name[index] form
            if let Some(bracket_pos) = operand.find('[')
                && operand.ends_with(']')
            {
                let name = &operand[..bracket_pos];
                let index = &operand[bracket_pos + 1..operand.len() - 1];
                // Strip quotes from index for assoc array keys
                let index_clean = if (index.starts_with('"') && index.ends_with('"'))
                    || (index.starts_with('\'') && index.ends_with('\''))
                {
                    &index[1..index.len() - 1]
                } else {
                    index
                };
                if let Some(vars) = ctx.variables {
                    if let Some(var) = vars.get(name) {
                        return Some(match &var.value {
                            crate::interpreter::VariableValue::IndexedArray(map) => {
                                let idx = eval_index_expr(index_clean, vars);
                                if idx < 0 {
                                    let max_key = map.keys().next_back().copied().unwrap_or(0);
                                    let resolved = max_key as i64 + 1 + idx;
                                    resolved >= 0 && map.contains_key(&(resolved as usize))
                                } else {
                                    map.contains_key(&(idx as usize))
                                }
                            }
                            crate::interpreter::VariableValue::AssociativeArray(map) => {
                                // Expand simple $var references in the key.
                                let expanded = expand_simple_vars(index_clean, vars);
                                map.contains_key(&expanded)
                            }
                            crate::interpreter::VariableValue::Scalar(s) => {
                                index_clean == "0" && !s.is_empty()
                            }
                        });
                    }
                    return Some(false);
                }
            }
            Some(ctx.env.contains_key(operand))
        }
        _ => None,
    }
}

/// Try to evaluate a binary test. Returns None if `op` isn't a binary operator.
fn try_binary(left: &str, op: &str, right: &str, ctx: &CommandContext) -> Option<bool> {
    match op {
        // String comparisons
        "=" | "==" => Some(left == right),
        "!=" => Some(left != right),
        "<" => Some(left < right),
        ">" => Some(left > right),

        // Glob pattern matching (used in extended test context, but support in [ too)
        "=~" => {
            // Basic regex match in test command — not standard, return false
            Some(false)
        }

        // Numeric comparisons
        "-eq" => numeric_cmp(left, right, |a, b| a == b),
        "-ne" => numeric_cmp(left, right, |a, b| a != b),
        "-lt" => numeric_cmp(left, right, |a, b| a < b),
        "-le" => numeric_cmp(left, right, |a, b| a <= b),
        "-gt" => numeric_cmp(left, right, |a, b| a > b),
        "-ge" => numeric_cmp(left, right, |a, b| a >= b),

        // File comparisons using VFS metadata
        "-ef" => Some(file_same_device_and_inode(left, right, ctx)),
        "-nt" => Some(file_newer_than(left, right, ctx)),
        "-ot" => Some(file_newer_than(right, left, ctx)),

        _ => None,
    }
}

fn numeric_cmp(left: &str, right: &str, cmp: impl Fn(i64, i64) -> bool) -> Option<bool> {
    // test/[ treats all numbers as plain decimal — no octal or hex.
    let a = left.trim().parse::<i64>().ok()?;
    let b = right.trim().parse::<i64>().ok()?;
    Some(cmp(a, b))
}

/// Parse an integer in bash style: decimal, 0x hex, or 0-prefixed octal.
/// Returns None for invalid literals (e.g. "08" — looks octal but has invalid digits).
fn parse_bash_int(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return Some(0);
    }
    // Handle optional leading sign
    let (negative, s) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };
    let val = if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        i64::from_str_radix(hex, 16).ok()?
    } else if s.starts_with('0') && s.len() > 1 && s[1..].chars().all(|c| c.is_ascii_digit()) {
        // Starts with 0 and has more digits — must be valid octal.
        // If any digit is 8 or 9, from_str_radix will fail → None (bash errors on "08").
        i64::from_str_radix(s, 8).ok()?
    } else {
        s.parse::<i64>().ok()?
    };
    Some(if negative { -val } else { val })
}

// ── File test helpers ─────────────────────────────────────────────

/// Evaluate an array index expression using available variable context.
/// Handles: integer literals, simple variable names, and basic binary ops (+, -, *).
fn eval_index_expr(
    expr: &str,
    vars: &std::collections::HashMap<String, crate::interpreter::Variable>,
) -> i64 {
    let trimmed = expr.trim();
    // Integer literal
    if let Ok(n) = trimmed.parse::<i64>() {
        return n;
    }
    // Single variable name
    if trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return vars
            .get(trimmed)
            .map(|v| v.value.as_scalar().parse::<i64>().unwrap_or(0))
            .unwrap_or(0);
    }
    // Simple binary expression: look for +, -, * (not at start for unary minus)
    type BinOp = (char, fn(i64, i64) -> i64);
    let ops: [BinOp; 3] = [
        ('+', |a, b| a + b),
        ('-', |a, b| a - b),
        ('*', |a, b| a * b),
    ];
    for (ch, op) in ops {
        // Find the operator not at position 0
        if let Some(pos) = trimmed[1..].find(ch).map(|p| p + 1) {
            let left = eval_index_expr(&trimmed[..pos], vars);
            let right = eval_index_expr(&trimmed[pos + 1..], vars);
            return op(left, right);
        }
    }
    0
}

/// Expand simple `$name` references in a string using the variable context.
fn expand_simple_vars(
    s: &str,
    vars: &std::collections::HashMap<String, crate::interpreter::Variable>,
) -> String {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$' && i + 1 < chars.len() {
            i += 1;
            let mut name = String::new();
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                name.push(chars[i]);
                i += 1;
            }
            if let Some(var) = vars.get(&name) {
                result.push_str(var.value.as_scalar());
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn resolve_test_path(path_str: &str, ctx: &CommandContext) -> String {
    if path_str.starts_with('/') {
        path_str.to_string()
    } else {
        format!("{}/{}", ctx.cwd.trim_end_matches('/'), path_str)
    }
}

fn file_exists(path_str: &str, ctx: &CommandContext) -> bool {
    let resolved = resolve_test_path(path_str, ctx);
    ctx.fs.exists(Path::new(&resolved))
}

fn file_is_regular(path_str: &str, ctx: &CommandContext) -> bool {
    let resolved = resolve_test_path(path_str, ctx);
    ctx.fs
        .stat(Path::new(&resolved))
        .map(|m| m.node_type == NodeType::File)
        .unwrap_or(false)
}

fn file_is_dir(path_str: &str, ctx: &CommandContext) -> bool {
    let resolved = resolve_test_path(path_str, ctx);
    ctx.fs
        .stat(Path::new(&resolved))
        .map(|m| m.node_type == NodeType::Directory)
        .unwrap_or(false)
}

fn file_is_symlink(path_str: &str, ctx: &CommandContext) -> bool {
    let resolved = resolve_test_path(path_str, ctx);
    ctx.fs
        .lstat(Path::new(&resolved))
        .map(|m| m.node_type == NodeType::Symlink)
        .unwrap_or(false)
}

fn file_size_nonzero(path_str: &str, ctx: &CommandContext) -> bool {
    let resolved = resolve_test_path(path_str, ctx);
    ctx.fs
        .stat(Path::new(&resolved))
        .map(|m| m.size > 0)
        .unwrap_or(false)
}

/// `-ef`: true if both paths resolve to the same file (same path after resolution).
fn file_same_device_and_inode(left: &str, right: &str, ctx: &CommandContext) -> bool {
    let l = resolve_test_path(left, ctx);
    let r = resolve_test_path(right, ctx);
    // In our VFS there are no real inodes; two paths are "the same file" if they
    // resolve to the same canonical path and the file exists.
    if !ctx.fs.exists(Path::new(&l)) || !ctx.fs.exists(Path::new(&r)) {
        return false;
    }
    // Canonicalize both paths through the VFS
    let lc = ctx
        .fs
        .canonicalize(Path::new(&l))
        .unwrap_or_else(|_| PathBuf::from(&l));
    let rc = ctx
        .fs
        .canonicalize(Path::new(&r))
        .unwrap_or_else(|_| PathBuf::from(&r));
    lc == rc
}

/// `-nt`: true if left is newer than right (or left exists and right does not).
fn file_newer_than(left: &str, right: &str, ctx: &CommandContext) -> bool {
    let l = resolve_test_path(left, ctx);
    let r = resolve_test_path(right, ctx);
    let l_meta = ctx.fs.stat(Path::new(&l));
    let r_meta = ctx.fs.stat(Path::new(&r));
    match (l_meta, r_meta) {
        (Ok(lm), Ok(rm)) => lm.mtime > rm.mtime,
        (Ok(_), Err(_)) => true, // left exists, right doesn't
        _ => false,
    }
}

/// `-o optname`: true if the named shell option is currently enabled.
fn is_shell_option_set(name: &str, ctx: &CommandContext) -> bool {
    if let Some(opts) = &ctx.shell_opts {
        match name {
            "errexit" | "errtrace" => opts.errexit,
            "nounset" => opts.nounset,
            "pipefail" => opts.pipefail,
            "xtrace" => opts.xtrace,
            "verbose" => opts.verbose,
            "noexec" => opts.noexec,
            "noclobber" => opts.noclobber,
            "allexport" => opts.allexport,
            "noglob" => opts.noglob,
            "posix" => opts.posix,
            "vi" => opts.vi_mode,
            "emacs" => opts.emacs_mode,
            _ => false,
        }
    } else {
        false
    }
}

// ── Shared helpers for extended test (used by walker.rs) ──────────

/// Evaluate a unary predicate on a path/string for `[[ ]]`.
pub(crate) fn eval_unary_predicate(
    pred: &brush_parser::ast::UnaryPredicate,
    operand: &str,
    fs: &dyn VirtualFs,
    cwd: &str,
    env: &std::collections::HashMap<String, String>,
    shell_opts: Option<&crate::interpreter::ShellOpts>,
) -> bool {
    use brush_parser::ast::UnaryPredicate::*;

    let resolve = |s: &str| -> String {
        if s.starts_with('/') {
            s.to_string()
        } else {
            format!("{}/{}", cwd.trim_end_matches('/'), s)
        }
    };

    match pred {
        FileExists => fs.exists(Path::new(&resolve(operand))),
        FileExistsAndIsRegularFile => fs
            .stat(Path::new(&resolve(operand)))
            .map(|m| m.node_type == NodeType::File)
            .unwrap_or(false),
        FileExistsAndIsDir => fs
            .stat(Path::new(&resolve(operand)))
            .map(|m| m.node_type == NodeType::Directory)
            .unwrap_or(false),
        FileExistsAndIsSymlink => fs
            .lstat(Path::new(&resolve(operand)))
            .map(|m| m.node_type == NodeType::Symlink)
            .unwrap_or(false),
        FileExistsAndIsReadable | FileExistsAndIsWritable | FileExistsAndIsExecutable => {
            fs.exists(Path::new(&resolve(operand)))
        }
        FileExistsAndIsNotZeroLength => fs
            .stat(Path::new(&resolve(operand)))
            .map(|m| m.size > 0)
            .unwrap_or(false),
        StringHasZeroLength => operand.is_empty(),
        StringHasNonZeroLength => !operand.is_empty(),
        ShellVariableIsSetAndAssigned => env.contains_key(operand),
        ShellOptionEnabled => {
            if let Some(opts) = shell_opts {
                match operand {
                    "errexit" | "errtrace" => opts.errexit,
                    "nounset" => opts.nounset,
                    "pipefail" => opts.pipefail,
                    "xtrace" => opts.xtrace,
                    "verbose" => opts.verbose,
                    "noexec" => opts.noexec,
                    "noclobber" => opts.noclobber,
                    "allexport" => opts.allexport,
                    "noglob" => opts.noglob,
                    "posix" => opts.posix,
                    "vi" => opts.vi_mode,
                    "emacs" => opts.emacs_mode,
                    _ => false,
                }
            } else {
                false
            }
        }
        // -O/-G: always true if file exists (VFS has no user concept)
        FileExistsAndOwnedByEffectiveGroupId | FileExistsAndOwnedByEffectiveUserId => {
            fs.exists(Path::new(&resolve(operand)))
        }
        // Unsupported file tests
        FileExistsAndIsBlockSpecialFile
        | FileExistsAndIsCharSpecialFile
        | FileExistsAndIsSetgid
        | FileExistsAndHasStickyBit
        | FileExistsAndIsFifo
        | FdIsOpenTerminal
        | FileExistsAndIsSetuid
        | FileExistsAndModifiedSinceLastRead
        | FileExistsAndIsSocket
        | ShellVariableIsSetAndNameRef => false,
    }
}

/// Evaluate a binary predicate for `[[ ]]`.
/// `pattern_match` controls whether == and != use glob matching (true in [[, false in [).
pub(crate) fn eval_binary_predicate(
    pred: &brush_parser::ast::BinaryPredicate,
    left: &str,
    right: &str,
    pattern_match: bool,
    fs: &dyn VirtualFs,
    cwd: &str,
) -> bool {
    use brush_parser::ast::BinaryPredicate::*;

    let resolve = |s: &str| -> String {
        if s.starts_with('/') {
            s.to_string()
        } else {
            format!("{}/{}", cwd.trim_end_matches('/'), s)
        }
    };

    match pred {
        StringExactlyMatchesString => left == right,
        StringDoesNotExactlyMatchString => left != right,
        StringExactlyMatchesPattern => {
            if pattern_match {
                glob_match(right, left)
            } else {
                left == right
            }
        }
        StringDoesNotExactlyMatchPattern => {
            if pattern_match {
                !glob_match(right, left)
            } else {
                left != right
            }
        }
        LeftSortsBeforeRight => left < right,
        LeftSortsAfterRight => left > right,
        ArithmeticEqualTo => parse_nums(left, right).is_some_and(|(a, b)| a == b),
        ArithmeticNotEqualTo => parse_nums(left, right).is_some_and(|(a, b)| a != b),
        ArithmeticLessThan => parse_nums(left, right).is_some_and(|(a, b)| a < b),
        ArithmeticLessThanOrEqualTo => parse_nums(left, right).is_some_and(|(a, b)| a <= b),
        ArithmeticGreaterThan => parse_nums(left, right).is_some_and(|(a, b)| a > b),
        ArithmeticGreaterThanOrEqualTo => parse_nums(left, right).is_some_and(|(a, b)| a >= b),
        // Regex matching handled separately in extended test
        StringMatchesRegex | StringContainsSubstring => false,
        // File comparisons using VFS metadata
        FilesReferToSameDeviceAndInodeNumbers => {
            let l = resolve(left);
            let r = resolve(right);
            if !fs.exists(Path::new(&l)) || !fs.exists(Path::new(&r)) {
                return false;
            }
            let lc = fs
                .canonicalize(Path::new(&l))
                .unwrap_or_else(|_| PathBuf::from(&l));
            let rc = fs
                .canonicalize(Path::new(&r))
                .unwrap_or_else(|_| PathBuf::from(&r));
            lc == rc
        }
        LeftFileIsNewerOrExistsWhenRightDoesNot => {
            let l = resolve(left);
            let r = resolve(right);
            match (fs.stat(Path::new(&l)), fs.stat(Path::new(&r))) {
                (Ok(lm), Ok(rm)) => lm.mtime > rm.mtime,
                (Ok(_), Err(_)) => true,
                _ => false,
            }
        }
        LeftFileIsOlderOrDoesNotExistWhenRightDoes => {
            let l = resolve(left);
            let r = resolve(right);
            match (fs.stat(Path::new(&l)), fs.stat(Path::new(&r))) {
                (Ok(lm), Ok(rm)) => lm.mtime < rm.mtime,
                (Err(_), Ok(_)) => true,
                _ => false,
            }
        }
    }
}

fn parse_nums(a: &str, b: &str) -> Option<(i64, i64)> {
    Some((parse_bash_int(a)?, parse_bash_int(b)?))
}

// ── Result helpers ────────────────────────────────────────────────

fn result(exit_code: i32) -> CommandResult {
    CommandResult {
        exit_code,
        ..CommandResult::default()
    }
}

fn error_result(msg: &str) -> CommandResult {
    CommandResult {
        stderr: format!("test: {msg}\n"),
        exit_code: 2,
        ..CommandResult::default()
    }
}
