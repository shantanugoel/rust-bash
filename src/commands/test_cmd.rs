//! Implementation of the `test` and `[` shell commands.
//!
//! Evaluates conditional expressions for file tests, string comparisons,
//! numeric comparisons, and logical operators. Returns exit code 0 for
//! true, 1 for false, and 2 for usage errors.

use crate::commands::{CommandContext, CommandResult};
use crate::interpreter::pattern::glob_match;
use crate::vfs::{NodeType, VirtualFs};
use std::path::Path;

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
            return Err("argument expected".to_string());
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
        "-e" => Some(file_exists(operand, ctx)),
        "-f" => Some(file_is_regular(operand, ctx)),
        "-d" => Some(file_is_dir(operand, ctx)),
        "-L" | "-h" => Some(file_is_symlink(operand, ctx)),
        "-s" => Some(file_size_nonzero(operand, ctx)),
        "-r" => Some(file_exists(operand, ctx)), // always readable in VFS
        "-w" => Some(file_exists(operand, ctx)), // always writable in VFS
        "-x" => Some(file_exists(operand, ctx)), // always executable in VFS
        "-b" | "-c" | "-p" | "-S" | "-u" | "-g" | "-k" | "-t" | "-G" | "-N" | "-O" => {
            Some(false) // unsupported file tests always false
        }
        "-v" => {
            // Shell variable is set — we check via env in context
            Some(ctx.env.contains_key(operand))
        }
        _ => None,
    }
}

/// Try to evaluate a binary test. Returns None if `op` isn't a binary operator.
fn try_binary(left: &str, op: &str, right: &str, _ctx: &CommandContext) -> Option<bool> {
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

        // File comparisons (not yet implemented for VFS)
        "-ef" | "-nt" | "-ot" => Some(false),

        _ => None,
    }
}

fn numeric_cmp(left: &str, right: &str, cmp: impl Fn(i64, i64) -> bool) -> Option<bool> {
    let a = left.parse::<i64>().ok()?;
    let b = right.parse::<i64>().ok()?;
    Some(cmp(a, b))
}

// ── File test helpers ─────────────────────────────────────────────

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

// ── Shared helpers for extended test (used by walker.rs) ──────────

/// Evaluate a unary predicate on a path/string for `[[ ]]`.
pub(crate) fn eval_unary_predicate(
    pred: &brush_parser::ast::UnaryPredicate,
    operand: &str,
    fs: &dyn VirtualFs,
    cwd: &str,
    env: &std::collections::HashMap<String, String>,
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
        ShellOptionEnabled => false,
        // Unsupported file tests
        FileExistsAndIsBlockSpecialFile
        | FileExistsAndIsCharSpecialFile
        | FileExistsAndIsSetgid
        | FileExistsAndHasStickyBit
        | FileExistsAndIsFifo
        | FdIsOpenTerminal
        | FileExistsAndIsSetuid
        | FileExistsAndOwnedByEffectiveGroupId
        | FileExistsAndModifiedSinceLastRead
        | FileExistsAndOwnedByEffectiveUserId
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
) -> bool {
    use brush_parser::ast::BinaryPredicate::*;

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
        // File comparisons not implemented
        FilesReferToSameDeviceAndInodeNumbers
        | LeftFileIsNewerOrExistsWhenRightDoesNot
        | LeftFileIsOlderOrDoesNotExistWhenRightDoes => false,
    }
}

fn parse_nums(a: &str, b: &str) -> Option<(i64, i64)> {
    Some((a.parse::<i64>().ok()?, b.parse::<i64>().ok()?))
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
