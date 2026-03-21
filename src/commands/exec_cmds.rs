//! Commands that use the exec callback: xargs, find

use crate::commands::{CommandContext, CommandResult};
use crate::interpreter::pattern::glob_match;
use crate::vfs::NodeType;
use std::path::{Path, PathBuf};

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

// ── xargs ────────────────────────────────────────────────────────────

pub struct XargsCommand;

impl super::VirtualCommand for XargsCommand {
    fn name(&self) -> &str {
        "xargs"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut replace_str: Option<String> = None;
        let mut max_args: Option<usize> = None;
        let mut delimiter: Option<String> = None;
        let mut null_delim = false;
        let mut command_parts: Vec<String> = Vec::new();
        let mut opts_done = false;

        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if !opts_done && arg == "--" {
                opts_done = true;
                i += 1;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                match arg.as_str() {
                    "-I" => {
                        i += 1;
                        if i < args.len() {
                            replace_str = Some(args[i].clone());
                        } else {
                            return CommandResult {
                                stderr: "xargs: option requires an argument -- 'I'\n".into(),
                                exit_code: 1,
                                ..Default::default()
                            };
                        }
                    }
                    "-n" => {
                        i += 1;
                        if i < args.len() {
                            match args[i].parse::<usize>() {
                                Ok(n) if n > 0 => max_args = Some(n),
                                _ => {
                                    return CommandResult {
                                        stderr: format!(
                                            "xargs: invalid number for -n: '{}'\n",
                                            args[i]
                                        ),
                                        exit_code: 1,
                                        ..Default::default()
                                    };
                                }
                            }
                        } else {
                            return CommandResult {
                                stderr: "xargs: option requires an argument -- 'n'\n".into(),
                                exit_code: 1,
                                ..Default::default()
                            };
                        }
                    }
                    "-d" => {
                        i += 1;
                        if i < args.len() {
                            delimiter = Some(args[i].clone());
                        } else {
                            return CommandResult {
                                stderr: "xargs: option requires an argument -- 'd'\n".into(),
                                exit_code: 1,
                                ..Default::default()
                            };
                        }
                    }
                    "-0" => {
                        null_delim = true;
                    }
                    _ => {
                        // Unknown option — treat as start of command
                        opts_done = true;
                        command_parts.push(arg.clone());
                    }
                }
            } else {
                opts_done = true;
                command_parts.push(arg.clone());
            }
            i += 1;
        }

        // Default command is echo
        if command_parts.is_empty() {
            command_parts.push("echo".to_string());
        }

        let exec = match ctx.exec {
            Some(exec) => exec,
            None => {
                return CommandResult {
                    stderr: "xargs: exec callback not available\n".into(),
                    exit_code: 1,
                    ..Default::default()
                };
            }
        };

        // Split input into tokens
        let input = ctx.stdin;
        let tokens: Vec<String> = if null_delim {
            input.split('\0').map(|s| s.to_string()).collect()
        } else if let Some(ref delim) = delimiter {
            let d = if delim == "\\n" {
                '\n'
            } else if delim == "\\t" {
                '\t'
            } else if delim == "\\0" {
                '\0'
            } else {
                delim.chars().next().unwrap_or('\n')
            };
            input.split(d).map(|s| s.to_string()).collect()
        } else {
            // Default: split on whitespace (newlines and spaces)
            input.split_whitespace().map(|s| s.to_string()).collect()
        };

        // Filter out empty tokens
        let tokens: Vec<String> = tokens.into_iter().filter(|t| !t.is_empty()).collect();

        if tokens.is_empty() {
            // No input — with replace mode, do nothing; without, run command once with no args
            if replace_str.is_some() {
                return CommandResult::default();
            }
            // With no input and no replace, run the command with no extra args
            let cmd_line = shell_join(&command_parts);
            match exec(&cmd_line) {
                Ok(r) => return r,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("xargs: {}\n", e),
                        exit_code: 1,
                        ..Default::default()
                    };
                }
            }
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut last_exit = 0;

        if let Some(ref repl) = replace_str {
            // -I mode: one invocation per token, replace occurrences in command
            for token in &tokens {
                let cmd_line: Vec<String> = command_parts
                    .iter()
                    .map(|part| part.replace(repl.as_str(), token))
                    .collect();
                let cmd_str = shell_join(&cmd_line);
                match exec(&cmd_str) {
                    Ok(r) => {
                        stdout.push_str(&r.stdout);
                        stderr.push_str(&r.stderr);
                        last_exit = r.exit_code;
                    }
                    Err(e) => {
                        stderr.push_str(&format!("xargs: {}\n", e));
                        last_exit = 1;
                    }
                }
            }
        } else if let Some(n) = max_args {
            // -n mode: batch N args per invocation
            for chunk in tokens.chunks(n) {
                let mut cmd_line = command_parts.clone();
                cmd_line.extend(chunk.iter().cloned());
                let cmd_str = shell_join(&cmd_line);
                match exec(&cmd_str) {
                    Ok(r) => {
                        stdout.push_str(&r.stdout);
                        stderr.push_str(&r.stderr);
                        last_exit = r.exit_code;
                    }
                    Err(e) => {
                        stderr.push_str(&format!("xargs: {}\n", e));
                        last_exit = 1;
                    }
                }
            }
        } else {
            // Default: all args in one invocation
            let mut cmd_line = command_parts.clone();
            cmd_line.extend(tokens);
            let cmd_str = shell_join(&cmd_line);
            match exec(&cmd_str) {
                Ok(r) => {
                    stdout.push_str(&r.stdout);
                    stderr.push_str(&r.stderr);
                    last_exit = r.exit_code;
                }
                Err(e) => {
                    stderr.push_str(&format!("xargs: {}\n", e));
                    last_exit = 1;
                }
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: last_exit,
        }
    }
}

/// Shell-escape and join args into a command string for the exec callback.
fn shell_join(parts: &[String]) -> String {
    parts
        .iter()
        .map(|p| {
            if p.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') {
                // Wrap in single quotes, escaping existing single quotes
                format!("'{}'", p.replace('\'', "'\\''"))
            } else {
                p.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ── find ─────────────────────────────────────────────────────────────

pub struct FindCommand;

#[derive(Debug, Clone)]
enum FindExpr {
    Name(String),
    Type(char),
    Empty,
    Newer(String),
    Print,
    ExecEach(Vec<String>),
    ExecBatch(Vec<String>),
    Not(Box<FindExpr>),
    And(Box<FindExpr>, Box<FindExpr>),
    Or(Box<FindExpr>, Box<FindExpr>),
}

impl super::VirtualCommand for FindCommand {
    fn name(&self) -> &str {
        "find"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut paths: Vec<String> = Vec::new();
        let mut expr_args: Vec<String> = Vec::new();
        let mut in_expr = false;

        // Separate paths from expression arguments
        for arg in args {
            if in_expr {
                expr_args.push(arg.clone());
            } else if arg.starts_with('-') || arg == "!" || arg == "(" || arg == ")" {
                in_expr = true;
                expr_args.push(arg.clone());
            } else {
                paths.push(arg.clone());
            }
        }

        if paths.is_empty() {
            paths.push(".".to_string());
        }

        // Parse expression
        let opts = match parse_find_expr(&expr_args) {
            Ok(v) => v,
            Err(e) => {
                return CommandResult {
                    stderr: format!("find: {}\n", e),
                    exit_code: 1,
                    ..Default::default()
                };
            }
        };

        let mut out = FindOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
            batch_paths: Vec::new(),
        };

        for search_path in &paths {
            let abs_path = resolve_path(search_path, ctx.cwd);
            let display_prefix = search_path.to_string();

            if !ctx.fs.exists(&abs_path) {
                out.stderr.push_str(&format!(
                    "find: '{}': No such file or directory\n",
                    search_path
                ));
                out.exit_code = 1;
                continue;
            }

            walk_find(ctx, &abs_path, &display_prefix, 0, &opts, &mut out);
        }

        // Execute batched -exec commands
        if !out.batch_paths.is_empty() {
            let paths = out.batch_paths.clone();
            execute_batched(ctx, &opts.expr, &paths, &mut out);
        }

        CommandResult {
            stdout: out.stdout,
            stderr: out.stderr,
            exit_code: out.exit_code,
        }
    }
}

/// Parsed options extracted from find arguments.
struct FindOpts {
    expr: Option<FindExpr>,
    max_depth: Option<usize>,
    min_depth: Option<usize>,
}

/// Mutable state accumulated during a find walk.
struct FindOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
    batch_paths: Vec<String>,
}

/// Parse find expression arguments into a tree.
fn parse_find_expr(args: &[String]) -> Result<FindOpts, String> {
    let mut max_depth: Option<usize> = None;
    let mut min_depth: Option<usize> = None;

    // First pass: extract global options (maxdepth, mindepth)
    let mut filtered: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-maxdepth" => {
                i += 1;
                if i >= args.len() {
                    return Err("missing argument to '-maxdepth'".into());
                }
                max_depth = Some(
                    args[i]
                        .parse::<usize>()
                        .map_err(|_| format!("invalid argument '{}' to '-maxdepth'", args[i]))?,
                );
            }
            "-mindepth" => {
                i += 1;
                if i >= args.len() {
                    return Err("missing argument to '-mindepth'".into());
                }
                min_depth = Some(
                    args[i]
                        .parse::<usize>()
                        .map_err(|_| format!("invalid argument '{}' to '-mindepth'", args[i]))?,
                );
            }
            _ => filtered.push(args[i].clone()),
        }
        i += 1;
    }

    if filtered.is_empty() {
        return Ok(FindOpts {
            expr: None,
            max_depth,
            min_depth,
        });
    }

    let (expr, pos) = parse_or_expr(&filtered, 0)?;
    if pos < filtered.len() {
        return Err(format!("unexpected argument '{}'", filtered[pos]));
    }

    Ok(FindOpts {
        expr: Some(expr),
        max_depth,
        min_depth,
    })
}

/// Parse OR expression: expr1 -o expr2
fn parse_or_expr(args: &[String], pos: usize) -> Result<(FindExpr, usize), String> {
    let (mut left, mut pos) = parse_and_expr(args, pos)?;

    while pos < args.len() && (args[pos] == "-o" || args[pos] == "-or") {
        pos += 1;
        let (right, new_pos) = parse_and_expr(args, pos)?;
        left = FindExpr::Or(Box::new(left), Box::new(right));
        pos = new_pos;
    }

    Ok((left, pos))
}

/// Parse AND expression: expr1 [-a] expr2
fn parse_and_expr(args: &[String], pos: usize) -> Result<(FindExpr, usize), String> {
    let (mut left, mut pos) = parse_unary_expr(args, pos)?;

    loop {
        if pos >= args.len() {
            break;
        }
        // Explicit -a / -and
        if args[pos] == "-a" || args[pos] == "-and" {
            pos += 1;
            let (right, new_pos) = parse_unary_expr(args, pos)?;
            left = FindExpr::And(Box::new(left), Box::new(right));
            pos = new_pos;
            continue;
        }
        // Implicit AND: next token is a primary (not -o, not ")")
        if args[pos] != "-o" && args[pos] != "-or" && args[pos] != ")" {
            let (right, new_pos) = parse_unary_expr(args, pos)?;
            left = FindExpr::And(Box::new(left), Box::new(right));
            pos = new_pos;
            continue;
        }
        break;
    }

    Ok((left, pos))
}

/// Parse unary: -not / ! or primary
fn parse_unary_expr(args: &[String], pos: usize) -> Result<(FindExpr, usize), String> {
    if pos >= args.len() {
        return Err("expected expression".into());
    }

    if args[pos] == "-not" || args[pos] == "!" {
        let (inner, pos) = parse_unary_expr(args, pos + 1)?;
        return Ok((FindExpr::Not(Box::new(inner)), pos));
    }

    if args[pos] == "(" {
        let (expr, pos) = parse_or_expr(args, pos + 1)?;
        if pos >= args.len() || args[pos] != ")" {
            return Err("missing closing ')'".into());
        }
        return Ok((expr, pos + 1));
    }

    parse_primary(args, pos)
}

/// Parse a single predicate or action.
fn parse_primary(args: &[String], pos: usize) -> Result<(FindExpr, usize), String> {
    if pos >= args.len() {
        return Err("expected expression".into());
    }

    match args[pos].as_str() {
        "-name" => {
            if pos + 1 >= args.len() {
                return Err("missing argument to '-name'".into());
            }
            Ok((FindExpr::Name(args[pos + 1].clone()), pos + 2))
        }
        "-type" => {
            if pos + 1 >= args.len() {
                return Err("missing argument to '-type'".into());
            }
            let t = args[pos + 1].chars().next().unwrap_or('f');
            Ok((FindExpr::Type(t), pos + 2))
        }
        "-empty" => Ok((FindExpr::Empty, pos + 1)),
        "-newer" => {
            if pos + 1 >= args.len() {
                return Err("missing argument to '-newer'".into());
            }
            Ok((FindExpr::Newer(args[pos + 1].clone()), pos + 2))
        }
        "-print" => Ok((FindExpr::Print, pos + 1)),
        "-exec" => {
            // Collect args until \; or +
            let mut cmd_parts = Vec::new();
            let mut j = pos + 1;
            let mut batch = false;
            loop {
                if j >= args.len() {
                    return Err("missing argument to '-exec'".into());
                }
                if args[j] == ";" {
                    break;
                }
                if args[j] == "+" && !cmd_parts.is_empty() {
                    batch = true;
                    break;
                }
                cmd_parts.push(args[j].clone());
                j += 1;
            }
            if batch {
                Ok((FindExpr::ExecBatch(cmd_parts), j + 1))
            } else {
                Ok((FindExpr::ExecEach(cmd_parts), j + 1))
            }
        }
        other => Err(format!("unknown predicate '{}'", other)),
    }
}

/// Recursively walk the filesystem, evaluating the find expression.
fn walk_find(
    ctx: &CommandContext,
    abs_path: &Path,
    display_path: &str,
    depth: usize,
    opts: &FindOpts,
    out: &mut FindOutput,
) {
    // Check max_depth before doing anything
    if opts.max_depth.is_some_and(|max| depth > max) {
        return;
    }

    // Evaluate expression on current path (respecting min_depth)
    let at_or_below_min = opts.min_depth.is_none() || depth >= opts.min_depth.unwrap();

    if at_or_below_min {
        let matched = match opts.expr {
            Some(ref e) => eval_find(ctx, abs_path, display_path, e, out),
            None => true,
        };

        // Default action: -print if no action in expression
        if matched && !has_action(&opts.expr) {
            out.stdout.push_str(display_path);
            out.stdout.push('\n');
        }
    }

    // Recurse into directories
    let meta = match ctx.fs.stat(abs_path) {
        Ok(m) => m,
        Err(_) => return,
    };

    if meta.node_type == NodeType::Directory {
        if opts.max_depth.is_some_and(|max| depth >= max) {
            return;
        }

        let mut entries = match ctx.fs.readdir(abs_path) {
            Ok(e) => e,
            Err(e) => {
                out.stderr
                    .push_str(&format!("find: '{}': {}\n", display_path, e));
                out.exit_code = 1;
                return;
            }
        };
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        for entry in entries {
            let child_abs = abs_path.join(&entry.name);
            let child_display = if display_path == "/" {
                format!("/{}", entry.name)
            } else {
                format!("{}/{}", display_path, entry.name)
            };
            walk_find(ctx, &child_abs, &child_display, depth + 1, opts, out);
        }
    }
}

/// Check if the expression tree contains any action (-print, -exec).
fn has_action(expr: &Option<FindExpr>) -> bool {
    match expr {
        None => false,
        Some(e) => expr_has_action(e),
    }
}

fn expr_has_action(expr: &FindExpr) -> bool {
    match expr {
        FindExpr::Print | FindExpr::ExecEach(_) | FindExpr::ExecBatch(_) => true,
        FindExpr::Not(inner) => expr_has_action(inner),
        FindExpr::And(a, b) | FindExpr::Or(a, b) => expr_has_action(a) || expr_has_action(b),
        _ => false,
    }
}

/// Evaluate a find expression against a path. Returns true if the path matches.
fn eval_find(
    ctx: &CommandContext,
    abs_path: &Path,
    display_path: &str,
    expr: &FindExpr,
    out: &mut FindOutput,
) -> bool {
    match expr {
        FindExpr::Name(pattern) => {
            let filename = abs_path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // For root path "/", filename is empty; use "/"
            let filename = if filename.is_empty() && display_path == "/" {
                "/".to_string()
            } else {
                filename
            };
            glob_match(pattern, &filename)
        }
        FindExpr::Type(t) => {
            let meta = match ctx.fs.stat(abs_path) {
                Ok(m) => m,
                Err(_) => return false,
            };
            match t {
                'f' => meta.node_type == NodeType::File,
                'd' => meta.node_type == NodeType::Directory,
                'l' => match ctx.fs.lstat(abs_path) {
                    Ok(m) => m.node_type == NodeType::Symlink,
                    Err(_) => false,
                },
                _ => false,
            }
        }
        FindExpr::Empty => {
            let meta = match ctx.fs.stat(abs_path) {
                Ok(m) => m,
                Err(_) => return false,
            };
            match meta.node_type {
                NodeType::File => meta.size == 0,
                NodeType::Directory => match ctx.fs.readdir(abs_path) {
                    Ok(entries) => entries.is_empty(),
                    Err(_) => false,
                },
                _ => false,
            }
        }
        FindExpr::Newer(ref_file) => {
            let ref_path = resolve_path(ref_file, ctx.cwd);
            let ref_meta = match ctx.fs.stat(&ref_path) {
                Ok(m) => m,
                Err(_) => return false,
            };
            let cur_meta = match ctx.fs.stat(abs_path) {
                Ok(m) => m,
                Err(_) => return false,
            };
            cur_meta.mtime > ref_meta.mtime
        }
        FindExpr::Print => {
            out.stdout.push_str(display_path);
            out.stdout.push('\n');
            true
        }
        FindExpr::ExecEach(cmd_parts) => {
            if let Some(exec) = ctx.exec {
                let cmd_str = cmd_parts
                    .iter()
                    .map(|p| {
                        if p == "{}" {
                            shell_escape(display_path)
                        } else {
                            shell_escape(p)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                match exec(&cmd_str) {
                    Ok(r) => {
                        out.stdout.push_str(&r.stdout);
                        out.stderr.push_str(&r.stderr);
                        if r.exit_code != 0 {
                            out.exit_code = r.exit_code;
                        }
                        r.exit_code == 0
                    }
                    Err(e) => {
                        out.stderr.push_str(&format!("find: exec error: {}\n", e));
                        out.exit_code = 1;
                        false
                    }
                }
            } else {
                out.stderr.push_str("find: exec callback not available\n");
                out.exit_code = 1;
                false
            }
        }
        FindExpr::ExecBatch(_) => {
            out.batch_paths.push(display_path.to_string());
            true
        }
        FindExpr::Not(inner) => !eval_find(ctx, abs_path, display_path, inner, out),
        FindExpr::And(a, b) => {
            if !eval_find(ctx, abs_path, display_path, a, out) {
                return false;
            }
            eval_find(ctx, abs_path, display_path, b, out)
        }
        FindExpr::Or(a, b) => {
            if eval_find(ctx, abs_path, display_path, a, out) {
                return true;
            }
            eval_find(ctx, abs_path, display_path, b, out)
        }
    }
}

/// Execute a batched -exec + command with all collected paths.
fn execute_batched(
    ctx: &CommandContext,
    expr: &Option<FindExpr>,
    paths: &[String],
    out: &mut FindOutput,
) {
    if let Some(expr) = expr {
        collect_batch_cmds(ctx, expr, paths, out);
    }
}

fn collect_batch_cmds(
    ctx: &CommandContext,
    expr: &FindExpr,
    paths: &[String],
    out: &mut FindOutput,
) {
    match expr {
        FindExpr::ExecBatch(cmd_parts) => {
            if let Some(exec) = ctx.exec {
                let has_placeholder = cmd_parts.iter().any(|p| p == "{}");
                let cmd_str = if has_placeholder {
                    let all_paths = paths
                        .iter()
                        .map(|p| shell_escape(p))
                        .collect::<Vec<_>>()
                        .join(" ");
                    cmd_parts
                        .iter()
                        .map(|p| {
                            if p == "{}" {
                                all_paths.clone()
                            } else {
                                shell_escape(p)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                } else {
                    let mut parts: Vec<String> =
                        cmd_parts.iter().map(|p| shell_escape(p)).collect();
                    parts.extend(paths.iter().map(|p| shell_escape(p)));
                    parts.join(" ")
                };
                match exec(&cmd_str) {
                    Ok(r) => {
                        out.stdout.push_str(&r.stdout);
                        out.stderr.push_str(&r.stderr);
                        if r.exit_code != 0 {
                            out.exit_code = r.exit_code;
                        }
                    }
                    Err(e) => {
                        out.stderr.push_str(&format!("find: exec error: {}\n", e));
                        out.exit_code = 1;
                    }
                }
            }
        }
        FindExpr::And(a, b) | FindExpr::Or(a, b) => {
            collect_batch_cmds(ctx, a, paths, out);
            collect_batch_cmds(ctx, b, paths, out);
        }
        FindExpr::Not(inner) => {
            collect_batch_cmds(ctx, inner, paths, out);
        }
        _ => {}
    }
}

fn shell_escape(s: &str) -> String {
    if s.contains(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '\\') {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandContext, CommandResult, ExecCallback, VirtualCommand};
    use crate::interpreter::ExecutionLimits;
    use crate::network::NetworkPolicy;
    use crate::vfs::{InMemoryFs, VirtualFs};
    use std::collections::HashMap;
    use std::sync::Arc;

    fn setup() -> (
        Arc<InMemoryFs>,
        HashMap<String, String>,
        ExecutionLimits,
        NetworkPolicy,
    ) {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(Path::new("/a.txt"), b"hello\n").unwrap();
        fs.write_file(Path::new("/b.md"), b"world\n").unwrap();
        fs.mkdir_p(Path::new("/dir1")).unwrap();
        fs.write_file(Path::new("/dir1/c.txt"), b"foo\n").unwrap();
        fs.mkdir_p(Path::new("/emptydir")).unwrap();
        (
            fs,
            HashMap::new(),
            ExecutionLimits::default(),
            NetworkPolicy::default(),
        )
    }

    fn ctx_with_exec<'a>(
        fs: &'a dyn VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        network_policy: &'a NetworkPolicy,
        stdin: &'a str,
        exec: Option<ExecCallback<'a>>,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin,
            limits,
            network_policy,
            exec,
        }
    }

    fn simple_exec(cmd: &str) -> Result<CommandResult, crate::error::RustBashError> {
        // Simple exec that handles "echo ..." and "cat ..." for testing.
        // Understands single-quoted arguments for proper shell_join handling.
        let parts = parse_simple_args(cmd);
        if parts.is_empty() {
            return Ok(CommandResult::default());
        }
        match parts[0].as_str() {
            "echo" => {
                let output = parts[1..].join(" ");
                Ok(CommandResult {
                    stdout: format!("{}\n", output),
                    ..Default::default()
                })
            }
            "cat" => {
                let output = parts[1..].join(" ");
                Ok(CommandResult {
                    stdout: format!("[cat:{}]\n", output),
                    ..Default::default()
                })
            }
            _ => Ok(CommandResult {
                stdout: format!("[{}]\n", cmd),
                ..Default::default()
            }),
        }
    }

    /// Parse a command string respecting single quotes.
    fn parse_simple_args(cmd: &str) -> Vec<String> {
        let mut args = Vec::new();
        let mut current = String::new();
        let mut in_single_quote = false;
        let chars = cmd.chars();

        for c in chars {
            if in_single_quote {
                if c == '\'' {
                    in_single_quote = false;
                } else {
                    current.push(c);
                }
            } else if c == '\'' {
                in_single_quote = true;
            } else if c.is_whitespace() {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            } else {
                current.push(c);
            }
        }
        if !current.is_empty() {
            args.push(current);
        }
        args
    }

    // ── xargs tests ──

    #[test]
    fn xargs_default_echo() {
        let (fs, env, limits, np) = setup();
        let exec_fn = simple_exec;
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "a\nb\nc\n", Some(&exec_fn));
        let r = XargsCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "a b c\n");
    }

    #[test]
    fn xargs_with_replace() {
        let (fs, env, limits, np) = setup();
        let exec_fn = simple_exec;
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "a\nb\nc\n", Some(&exec_fn));
        let r = XargsCommand.execute(
            &["-I".into(), "{}".into(), "echo".into(), "item: {}".into()],
            &c,
        );
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "item: a\nitem: b\nitem: c\n");
    }

    #[test]
    fn xargs_with_max_args() {
        let (fs, env, limits, np) = setup();
        let exec_fn = simple_exec;
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "1\n2\n3\n", Some(&exec_fn));
        let r = XargsCommand.execute(&["-n".into(), "1".into(), "echo".into(), "num:".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "num: 1\nnum: 2\nnum: 3\n");
    }

    #[test]
    fn xargs_null_delimited() {
        let (fs, env, limits, np) = setup();
        let exec_fn = simple_exec;
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "a\0b\0c", Some(&exec_fn));
        let r = XargsCommand.execute(&["-0".into(), "echo".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "a b c\n");
    }

    #[test]
    fn xargs_custom_delimiter() {
        let (fs, env, limits, np) = setup();
        let exec_fn = simple_exec;
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "a,b,c", Some(&exec_fn));
        let r = XargsCommand.execute(&["-d".into(), ",".into(), "echo".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "a b c\n");
    }

    // ── find tests ──

    #[test]
    fn find_all_from_root() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/a.txt"));
        assert!(r.stdout.contains("/b.md"));
        assert!(r.stdout.contains("/dir1"));
        assert!(r.stdout.contains("/dir1/c.txt"));
    }

    #[test]
    fn find_by_name_pattern() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/".into(), "-name".into(), "*.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/a.txt"));
        assert!(r.stdout.contains("/dir1/c.txt"));
        assert!(!r.stdout.contains("/b.md"));
    }

    #[test]
    fn find_type_directory() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/".into(), "-type".into(), "d".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/\n") || r.stdout.starts_with("/\n"));
        assert!(r.stdout.contains("/dir1"));
        assert!(r.stdout.contains("/emptydir"));
        assert!(!r.stdout.contains("/a.txt"));
    }

    #[test]
    fn find_type_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/".into(), "-type".into(), "f".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/a.txt"));
        assert!(r.stdout.contains("/b.md"));
        assert!(!r.stdout.contains("\n/\n"));
        assert!(!r.stdout.contains("\n/dir1\n"));
    }

    #[test]
    fn find_maxdepth() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/".into(), "-maxdepth".into(), "1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/a.txt"));
        assert!(r.stdout.contains("/dir1"));
        assert!(!r.stdout.contains("/dir1/c.txt"));
    }

    #[test]
    fn find_mindepth() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/".into(), "-mindepth".into(), "1".into()], &c);
        assert_eq!(r.exit_code, 0);
        // Should not include root itself
        let lines: Vec<&str> = r.stdout.lines().collect();
        assert!(!lines.contains(&"/"));
        assert!(r.stdout.contains("/a.txt"));
    }

    #[test]
    fn find_empty() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/empty.txt"), b"").unwrap();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/".into(), "-empty".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/empty.txt"));
        assert!(r.stdout.contains("/emptydir"));
    }

    #[test]
    fn find_not_name() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(
            &[
                "/".into(),
                "-type".into(),
                "f".into(),
                "-not".into(),
                "-name".into(),
                "*.txt".into(),
            ],
            &c,
        );
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/b.md"));
        assert!(!r.stdout.contains("/a.txt"));
        assert!(!r.stdout.contains("/dir1/c.txt"));
    }

    #[test]
    fn find_exec_each() {
        let (fs, env, limits, np) = setup();
        let exec_fn = simple_exec;
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", Some(&exec_fn));
        let r = FindCommand.execute(
            &[
                "/".into(),
                "-name".into(),
                "*.txt".into(),
                "-exec".into(),
                "cat".into(),
                "{}".into(),
                ";".into(),
            ],
            &c,
        );
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("[cat:/a.txt]"));
        assert!(r.stdout.contains("[cat:/dir1/c.txt]"));
    }

    #[test]
    fn find_or_expression() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(
            &[
                "/".into(),
                "-name".into(),
                "*.txt".into(),
                "-o".into(),
                "-name".into(),
                "*.md".into(),
            ],
            &c,
        );
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/a.txt"));
        assert!(r.stdout.contains("/b.md"));
        assert!(r.stdout.contains("/dir1/c.txt"));
    }

    #[test]
    fn find_nonexistent_path() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["/nonexistent".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("No such file or directory"));
    }

    #[test]
    fn find_default_path_is_dot() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_exec(&*fs, &env, &limits, &np, "", None);
        let r = FindCommand.execute(&["-type".into(), "f".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("a.txt"));
    }
}
