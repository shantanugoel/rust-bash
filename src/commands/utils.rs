//! Utility commands: expr, date, sleep, seq, env, printenv, which, base64,
//! md5sum, sha256sum, whoami, hostname, uname, yes

use crate::commands::{CommandContext, CommandResult};
use std::path::PathBuf;

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

// ── expr ─────────────────────────────────────────────────────────────

pub struct ExprCommand;

impl super::VirtualCommand for ExprCommand {
    fn name(&self) -> &str {
        "expr"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        if args.is_empty() {
            return CommandResult {
                stderr: "expr: missing operand\n".into(),
                exit_code: 2,
                ..Default::default()
            };
        }

        match eval_expr_tokens(args) {
            Ok(val) => {
                let exit_code = if val == "0" || val.is_empty() { 1 } else { 0 };
                CommandResult {
                    stdout: format!("{val}\n"),
                    exit_code,
                    ..Default::default()
                }
            }
            Err(e) => CommandResult {
                stderr: format!("expr: {e}\n"),
                exit_code: 2,
                ..Default::default()
            },
        }
    }
}

fn eval_expr_tokens(tokens: &[String]) -> Result<String, String> {
    // Handle special string operations first
    if tokens.len() >= 2 && tokens[0] == "length" {
        return Ok(tokens[1].len().to_string());
    }
    if tokens.len() >= 4 && tokens[0] == "substr" {
        let s = &tokens[1];
        let pos: usize = tokens[2]
            .parse()
            .map_err(|_| "non-integer argument".to_string())?;
        let len: usize = tokens[3]
            .parse()
            .map_err(|_| "non-integer argument".to_string())?;
        if pos == 0 {
            return Ok(String::new());
        }
        let start = pos.saturating_sub(1);
        let chars: Vec<char> = s.chars().collect();
        let end = (start + len).min(chars.len());
        let result: String = chars[start..end].iter().collect();
        return Ok(result);
    }
    if tokens.len() >= 3 && tokens[0] == "match" {
        return expr_match(&tokens[1], &tokens[2]);
    }

    let mut pos = 0;
    let result = parse_or(tokens, &mut pos)?;
    if pos != tokens.len() {
        return Err("syntax error".to_string());
    }
    Ok(result)
}

fn parse_or(tokens: &[String], pos: &mut usize) -> Result<String, String> {
    let mut left = parse_and(tokens, pos)?;
    while *pos < tokens.len() && tokens[*pos] == "|" {
        *pos += 1;
        let right = parse_and(tokens, pos)?;
        left = if is_truthy(&left) { left } else { right };
    }
    Ok(left)
}

fn parse_and(tokens: &[String], pos: &mut usize) -> Result<String, String> {
    let mut left = parse_comparison(tokens, pos)?;
    while *pos < tokens.len() && tokens[*pos] == "&" {
        *pos += 1;
        let right = parse_comparison(tokens, pos)?;
        left = if is_truthy(&left) && is_truthy(&right) {
            left
        } else {
            "0".to_string()
        };
    }
    Ok(left)
}

fn parse_comparison(tokens: &[String], pos: &mut usize) -> Result<String, String> {
    let left = parse_add(tokens, pos)?;
    if *pos < tokens.len() {
        let op = &tokens[*pos];
        match op.as_str() {
            "=" | "==" | "!=" | "<" | ">" | "<=" | ">=" => {
                *pos += 1;
                let right = parse_add(tokens, pos)?;
                let result = if let (Ok(l), Ok(r)) = (left.parse::<i64>(), right.parse::<i64>()) {
                    match op.as_str() {
                        "=" | "==" => l == r,
                        "!=" => l != r,
                        "<" => l < r,
                        ">" => l > r,
                        "<=" => l <= r,
                        ">=" => l >= r,
                        _ => false,
                    }
                } else {
                    match op.as_str() {
                        "=" | "==" => left == right,
                        "!=" => left != right,
                        "<" => left < right,
                        ">" => left > right,
                        "<=" => left <= right,
                        ">=" => left >= right,
                        _ => false,
                    }
                };
                return Ok(if result {
                    "1".to_string()
                } else {
                    "0".to_string()
                });
            }
            _ => {}
        }
    }
    Ok(left)
}

fn parse_add(tokens: &[String], pos: &mut usize) -> Result<String, String> {
    let mut left = parse_mul(tokens, pos)?;
    while *pos < tokens.len() && (tokens[*pos] == "+" || tokens[*pos] == "-") {
        let op = tokens[*pos].clone();
        *pos += 1;
        let right = parse_mul(tokens, pos)?;
        let l: i64 = left
            .parse()
            .map_err(|_| "non-integer argument".to_string())?;
        let r: i64 = right
            .parse()
            .map_err(|_| "non-integer argument".to_string())?;
        left = match op.as_str() {
            "+" => (l + r).to_string(),
            "-" => (l - r).to_string(),
            _ => unreachable!(),
        };
    }
    Ok(left)
}

fn parse_mul(tokens: &[String], pos: &mut usize) -> Result<String, String> {
    let mut left = parse_match(tokens, pos)?;
    while *pos < tokens.len() && (tokens[*pos] == "*" || tokens[*pos] == "/" || tokens[*pos] == "%")
    {
        let op = tokens[*pos].clone();
        *pos += 1;
        let right = parse_match(tokens, pos)?;
        let l: i64 = left
            .parse()
            .map_err(|_| "non-integer argument".to_string())?;
        let r: i64 = right
            .parse()
            .map_err(|_| "non-integer argument".to_string())?;
        if (op == "/" || op == "%") && r == 0 {
            return Err("division by zero".to_string());
        }
        left = match op.as_str() {
            "*" => (l * r).to_string(),
            "/" => (l / r).to_string(),
            "%" => (l % r).to_string(),
            _ => unreachable!(),
        };
    }
    Ok(left)
}

fn parse_match(tokens: &[String], pos: &mut usize) -> Result<String, String> {
    let left = parse_primary(tokens, pos)?;
    if *pos < tokens.len() && tokens[*pos] == ":" {
        *pos += 1;
        if *pos >= tokens.len() {
            return Err("syntax error".to_string());
        }
        let pattern = &tokens[*pos];
        *pos += 1;
        return expr_match(&left, pattern);
    }
    Ok(left)
}

fn parse_primary(tokens: &[String], pos: &mut usize) -> Result<String, String> {
    if *pos >= tokens.len() {
        return Err("syntax error".to_string());
    }
    if tokens[*pos] == "(" {
        *pos += 1;
        let val = parse_or(tokens, pos)?;
        if *pos >= tokens.len() || tokens[*pos] != ")" {
            return Err("syntax error: expecting ')'".to_string());
        }
        *pos += 1;
        return Ok(val);
    }
    let val = tokens[*pos].clone();
    *pos += 1;
    Ok(val)
}

fn expr_match(s: &str, pattern: &str) -> Result<String, String> {
    // expr match is anchored at the beginning
    let anchored = if pattern.starts_with('^') {
        pattern.to_string()
    } else {
        format!("^{pattern}")
    };
    let re = regex::Regex::new(&anchored).map_err(|e| format!("invalid regex: {e}"))?;
    if let Some(m) = re.captures(s) {
        if let Some(group) = m.get(1) {
            Ok(group.as_str().to_string())
        } else {
            Ok(m[0].len().to_string())
        }
    } else {
        // If regex has groups, return empty string, else return 0
        if anchored.contains('(') {
            Ok(String::new())
        } else {
            Ok("0".to_string())
        }
    }
}

fn is_truthy(s: &str) -> bool {
    !s.is_empty() && s != "0"
}

// ── date ─────────────────────────────────────────────────────────────

pub struct DateCommand;

impl super::VirtualCommand for DateCommand {
    fn name(&self) -> &str {
        "date"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let now = chrono::Local::now();

        let format_str = args.iter().find(|a| a.starts_with('+'));
        let output = if let Some(fmt) = format_str {
            let fmt_str = &fmt[1..]; // strip leading +
            now.format(fmt_str).to_string()
        } else {
            now.format("%a %b %e %H:%M:%S %Z %Y").to_string()
        };

        CommandResult {
            stdout: format!("{output}\n"),
            ..Default::default()
        }
    }
}

// ── sleep ────────────────────────────────────────────────────────────

pub struct SleepCommand;

impl super::VirtualCommand for SleepCommand {
    fn name(&self) -> &str {
        "sleep"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        if args.is_empty() {
            return CommandResult {
                stderr: "sleep: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let seconds: f64 = match args[0].parse() {
            Ok(v) => v,
            Err(_) => {
                return CommandResult {
                    stderr: format!("sleep: invalid time interval '{}'\n", args[0]),
                    exit_code: 1,
                    ..Default::default()
                };
            }
        };

        if seconds < 0.0 {
            return CommandResult {
                stderr: format!("sleep: invalid time interval '{}'\n", args[0]),
                exit_code: 1,
                ..Default::default()
            };
        }

        // Cap sleep to the execution time limit
        let max_secs = ctx.limits.max_execution_time.as_secs_f64();
        let capped = seconds.min(max_secs);
        let duration = std::time::Duration::from_secs_f64(capped);

        #[cfg(target_arch = "wasm32")]
        {
            let _ = duration;
            return CommandResult {
                stderr: "sleep: not supported in browser environment\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            std::thread::sleep(duration);
            CommandResult::default()
        }
    }
}

// ── seq ──────────────────────────────────────────────────────────────

pub struct SeqCommand;

impl super::VirtualCommand for SeqCommand {
    fn name(&self) -> &str {
        "seq"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let mut operands: Vec<&str> = Vec::new();
        let mut opts_done = false;

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                // Check if it's a negative number
                if arg[1..].starts_with(|c: char| c.is_ascii_digit() || c == '.') {
                    operands.push(arg);
                }
                // else ignore flags
            } else {
                operands.push(arg);
            }
        }

        if operands.is_empty() {
            return CommandResult {
                stderr: "seq: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let (first, increment, last) = match operands.len() {
            1 => {
                let last: f64 = match operands[0].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return CommandResult {
                            stderr: format!("seq: invalid argument '{}'\n", operands[0]),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                };
                (1.0, 1.0, last)
            }
            2 => {
                let first: f64 = match operands[0].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return CommandResult {
                            stderr: format!("seq: invalid argument '{}'\n", operands[0]),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                };
                let last: f64 = match operands[1].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return CommandResult {
                            stderr: format!("seq: invalid argument '{}'\n", operands[1]),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                };
                let inc = if first <= last { 1.0 } else { -1.0 };
                (first, inc, last)
            }
            _ => {
                let first: f64 = match operands[0].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return CommandResult {
                            stderr: format!("seq: invalid argument '{}'\n", operands[0]),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                };
                let inc: f64 = match operands[1].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return CommandResult {
                            stderr: format!("seq: invalid argument '{}'\n", operands[1]),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                };
                let last: f64 = match operands[2].parse() {
                    Ok(v) => v,
                    Err(_) => {
                        return CommandResult {
                            stderr: format!("seq: invalid argument '{}'\n", operands[2]),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                };
                (first, inc, last)
            }
        };

        if increment == 0.0 {
            return CommandResult {
                stderr: "seq: zero increment\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        // Determine if all args are integers for clean formatting
        let all_ints = operands.iter().all(|s| s.parse::<i64>().is_ok());

        let mut stdout = String::new();
        let mut current = first;
        let max_iters = 1_000_000usize; // safety limit
        let mut count = 0;

        loop {
            if increment > 0.0 && current > last + f64::EPSILON {
                break;
            }
            if increment < 0.0 && current < last - f64::EPSILON {
                break;
            }
            if count >= max_iters {
                break;
            }

            if all_ints {
                stdout.push_str(&format!("{}\n", current as i64));
            } else {
                // Format nicely: strip trailing zeros
                let s = format!("{current}");
                stdout.push_str(&s);
                stdout.push('\n');
            }

            current += increment;
            count += 1;
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── env ──────────────────────────────────────────────────────────────

pub struct EnvCommand;

impl super::VirtualCommand for EnvCommand {
    fn name(&self) -> &str {
        "env"
    }

    fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut stdout = String::new();
        let mut keys: Vec<&String> = ctx.env.keys().collect();
        keys.sort();
        for key in keys {
            if let Some(val) = ctx.env.get(key) {
                stdout.push_str(&format!("{key}={val}\n"));
            }
        }
        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── printenv ─────────────────────────────────────────────────────────

pub struct PrintenvCommand;

impl super::VirtualCommand for PrintenvCommand {
    fn name(&self) -> &str {
        "printenv"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        if args.is_empty() {
            // Same as env
            let mut stdout = String::new();
            let mut keys: Vec<&String> = ctx.env.keys().collect();
            keys.sort();
            for key in keys {
                if let Some(val) = ctx.env.get(key) {
                    stdout.push_str(&format!("{key}={val}\n"));
                }
            }
            return CommandResult {
                stdout,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut exit_code = 0;

        for arg in args {
            if let Some(val) = ctx.env.get(arg.as_str()) {
                stdout.push_str(val);
                stdout.push('\n');
            } else {
                exit_code = 1;
            }
        }

        CommandResult {
            stdout,
            exit_code,
            ..Default::default()
        }
    }
}

// ── which ────────────────────────────────────────────────────────────

pub struct WhichCommand;

const SHELL_BUILTINS: &[&str] = &[
    "cd",
    "export",
    "unset",
    "source",
    ".",
    "eval",
    "exec",
    "set",
    "shift",
    "return",
    "exit",
    "trap",
    "readonly",
    "declare",
    "local",
    "typeset",
    "let",
    "read",
    "mapfile",
    "readarray",
    "getopts",
    "hash",
    "type",
    "builtin",
    "command",
    "enable",
    "help",
    "logout",
    "times",
    "umask",
    "alias",
    "unalias",
    "bind",
    "complete",
    "compgen",
    "compopt",
    "dirs",
    "pushd",
    "popd",
    "shopt",
    "caller",
    "coproc",
    "wait",
    "jobs",
    "fg",
    "bg",
    "disown",
    "suspend",
    "kill",
];

const REGISTERED_COMMANDS: &[&str] = &[
    "echo",
    "true",
    "false",
    "cat",
    "pwd",
    "touch",
    "mkdir",
    "ls",
    "test",
    "[",
    "cp",
    "mv",
    "rm",
    "tee",
    "stat",
    "chmod",
    "ln",
    "grep",
    "sort",
    "uniq",
    "cut",
    "head",
    "tail",
    "wc",
    "tr",
    "rev",
    "fold",
    "nl",
    "printf",
    "paste",
    "realpath",
    "basename",
    "dirname",
    "tree",
    "expr",
    "date",
    "sleep",
    "seq",
    "env",
    "printenv",
    "which",
    "base64",
    "md5sum",
    "sha256sum",
    "whoami",
    "hostname",
    "uname",
    "yes",
];

impl super::VirtualCommand for WhichCommand {
    fn name(&self) -> &str {
        "which"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        if args.is_empty() {
            return CommandResult {
                stderr: "which: missing argument\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut exit_code = 0;

        for arg in args {
            if SHELL_BUILTINS.contains(&arg.as_str()) {
                stdout.push_str(&format!("{arg}: shell built-in command\n"));
            } else if REGISTERED_COMMANDS.contains(&arg.as_str()) {
                stdout.push_str(&format!("/usr/bin/{arg}\n"));
            } else {
                exit_code = 1;
            }
        }

        CommandResult {
            stdout,
            exit_code,
            ..Default::default()
        }
    }
}

// ── base64 ───────────────────────────────────────────────────────────

pub struct Base64Command;

impl super::VirtualCommand for Base64Command {
    fn name(&self) -> &str {
        "base64"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut decode = false;
        let mut wrap_width: Option<usize> = Some(76); // default line wrapping
        let mut opts_done = false;
        let mut files: Vec<&str> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            if !opts_done && arg == "--" {
                opts_done = true;
                i += 1;
                continue;
            }
            if !opts_done && (arg == "-d" || arg == "--decode") {
                decode = true;
            } else if !opts_done && arg.starts_with("-w") {
                let val = if arg.len() > 2 {
                    arg[2..].to_string()
                } else {
                    i += 1;
                    if i < args.len() {
                        args[i].clone()
                    } else {
                        "76".to_string()
                    }
                };
                let w: usize = val.parse().unwrap_or(76);
                wrap_width = if w == 0 { None } else { Some(w) };
            } else if !opts_done && arg == "-w" {
                i += 1;
                if i < args.len() {
                    let w: usize = args[i].parse().unwrap_or(76);
                    wrap_width = if w == 0 { None } else { Some(w) };
                }
            } else {
                files.push(arg);
            }
            i += 1;
        }

        let input = if files.is_empty() {
            ctx.stdin.as_bytes().to_vec()
        } else {
            let path = resolve_path(files[0], ctx.cwd);
            match ctx.fs.read_file(&path) {
                Ok(bytes) => bytes,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("base64: {}: {}\n", files[0], e),
                        exit_code: 1,
                        ..Default::default()
                    };
                }
            }
        };

        if decode {
            use base64::Engine;
            let input_str: String = input.iter().map(|&b| b as char).collect();
            let cleaned: String = input_str.chars().filter(|c| !c.is_whitespace()).collect();
            match base64::engine::general_purpose::STANDARD.decode(cleaned.as_bytes()) {
                Ok(decoded) => CommandResult {
                    stdout: String::from_utf8_lossy(&decoded).to_string(),
                    ..Default::default()
                },
                Err(e) => CommandResult {
                    stderr: format!("base64: invalid input: {e}\n"),
                    exit_code: 1,
                    ..Default::default()
                },
            }
        } else {
            use base64::Engine;
            let encoded = base64::engine::general_purpose::STANDARD.encode(&input);
            let stdout = match wrap_width {
                Some(w) if w > 0 => {
                    let mut wrapped = String::new();
                    for (i, c) in encoded.chars().enumerate() {
                        if i > 0 && i % w == 0 {
                            wrapped.push('\n');
                        }
                        wrapped.push(c);
                    }
                    wrapped.push('\n');
                    wrapped
                }
                _ => format!("{encoded}\n"),
            };
            CommandResult {
                stdout,
                ..Default::default()
            }
        }
    }
}

// ── md5sum ───────────────────────────────────────────────────────────

pub struct Md5sumCommand;

impl super::VirtualCommand for Md5sumCommand {
    fn name(&self) -> &str {
        "md5sum"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        use md5::Digest;

        let mut files: Vec<&str> = Vec::new();
        let mut opts_done = false;

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 && arg != "-" {
                // ignore flags
            } else {
                files.push(arg);
            }
        }

        if files.is_empty() {
            files.push("-");
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for file in &files {
            let data = if *file == "-" {
                ctx.stdin.as_bytes().to_vec()
            } else {
                let path = resolve_path(file, ctx.cwd);
                match ctx.fs.read_file(&path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        stderr.push_str(&format!("md5sum: {}: {}\n", file, e));
                        exit_code = 1;
                        continue;
                    }
                }
            };

            let mut hasher = md5::Md5::new();
            hasher.update(&data);
            let hash = hasher.finalize();
            let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
            let display_name = if *file == "-" { "-" } else { file };
            stdout.push_str(&format!("{}  {}\n", hex, display_name));
        }

        CommandResult {
            stdout,
            stderr,
            exit_code,
        }
    }
}

// ── sha256sum ────────────────────────────────────────────────────────

pub struct Sha256sumCommand;

impl super::VirtualCommand for Sha256sumCommand {
    fn name(&self) -> &str {
        "sha256sum"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        use sha2::Digest;

        let mut files: Vec<&str> = Vec::new();
        let mut opts_done = false;

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 && arg != "-" {
                // ignore flags
            } else {
                files.push(arg);
            }
        }

        if files.is_empty() {
            files.push("-");
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for file in &files {
            let data = if *file == "-" {
                ctx.stdin.as_bytes().to_vec()
            } else {
                let path = resolve_path(file, ctx.cwd);
                match ctx.fs.read_file(&path) {
                    Ok(bytes) => bytes,
                    Err(e) => {
                        stderr.push_str(&format!("sha256sum: {}: {}\n", file, e));
                        exit_code = 1;
                        continue;
                    }
                }
            };

            let mut hasher = sha2::Sha256::new();
            hasher.update(&data);
            let hash = hasher.finalize();
            let hex: String = hash.iter().map(|b| format!("{b:02x}")).collect();
            let display_name = if *file == "-" { "-" } else { file };
            stdout.push_str(&format!("{}  {}\n", hex, display_name));
        }

        CommandResult {
            stdout,
            stderr,
            exit_code,
        }
    }
}

// ── whoami ───────────────────────────────────────────────────────────

pub struct WhoamiCommand;

impl super::VirtualCommand for WhoamiCommand {
    fn name(&self) -> &str {
        "whoami"
    }

    fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
        let user = ctx
            .env
            .get("USER")
            .cloned()
            .unwrap_or_else(|| "root".to_string());
        CommandResult {
            stdout: format!("{user}\n"),
            ..Default::default()
        }
    }
}

// ── hostname ─────────────────────────────────────────────────────────

pub struct HostnameCommand;

impl super::VirtualCommand for HostnameCommand {
    fn name(&self) -> &str {
        "hostname"
    }

    fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
        let host = ctx
            .env
            .get("HOSTNAME")
            .cloned()
            .unwrap_or_else(|| "localhost".to_string());
        CommandResult {
            stdout: format!("{host}\n"),
            ..Default::default()
        }
    }
}

// ── uname ────────────────────────────────────────────────────────────

pub struct UnameCommand;

impl super::VirtualCommand for UnameCommand {
    fn name(&self) -> &str {
        "uname"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let mut show_sysname = false;
        let mut show_nodename = false;
        let mut show_release = false;
        let mut show_machine = false;
        let mut show_all = false;

        if args.is_empty() {
            show_sysname = true;
        }

        for arg in args {
            if let Some(flags) = arg.strip_prefix('-') {
                for c in flags.chars() {
                    match c {
                        'a' => show_all = true,
                        's' => show_sysname = true,
                        'n' => show_nodename = true,
                        'r' => show_release = true,
                        'm' => show_machine = true,
                        _ => {}
                    }
                }
            }
        }

        let sysname = "Linux";
        let nodename = "rust-bash";
        let release = "6.0.0-virtual";
        let machine = "x86_64";

        let mut parts = Vec::new();
        if show_all || show_sysname {
            parts.push(sysname);
        }
        if show_all || show_nodename {
            parts.push(nodename);
        }
        if show_all || show_release {
            parts.push(release);
        }
        if show_all {
            parts.push("#1 SMP");
        }
        if show_all || show_machine {
            parts.push(machine);
        }

        CommandResult {
            stdout: format!("{}\n", parts.join(" ")),
            ..Default::default()
        }
    }
}

// ── yes ──────────────────────────────────────────────────────────────

pub struct YesCommand;

impl super::VirtualCommand for YesCommand {
    fn name(&self) -> &str {
        "yes"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let text = if args.is_empty() {
            "y".to_string()
        } else {
            args.join(" ")
        };

        let max_lines = 10_000;
        let mut stdout = String::new();
        for _ in 0..max_lines {
            stdout.push_str(&text);
            stdout.push('\n');
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandContext, VirtualCommand};
    use crate::interpreter::ExecutionLimits;
    use crate::network::NetworkPolicy;
    use crate::vfs::{InMemoryFs, VirtualFs};
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    fn setup() -> (
        Arc<InMemoryFs>,
        HashMap<String, String>,
        ExecutionLimits,
        NetworkPolicy,
    ) {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(Path::new("/hello.txt"), b"hello world\n")
            .unwrap();
        let mut env = HashMap::new();
        env.insert("USER".into(), "testuser".into());
        env.insert("HOSTNAME".into(), "myhost".into());
        env.insert("HOME".into(), "/home/testuser".into());
        (
            fs,
            env,
            ExecutionLimits::default(),
            NetworkPolicy::default(),
        )
    }

    fn ctx<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        network_policy: &'a NetworkPolicy,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin: "",
            limits,
            network_policy,
            exec: None,
        }
    }

    fn ctx_with_stdin<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        network_policy: &'a NetworkPolicy,
        stdin: &'a str,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin,
            limits,
            network_policy,
            exec: None,
        }
    }

    // ── expr tests ───────────────────────────────────────────────────

    #[test]
    fn expr_addition() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["1".into(), "+".into(), "2".into()], &c);
        assert_eq!(r.stdout, "3\n");
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn expr_multiplication() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["3".into(), "*".into(), "4".into()], &c);
        assert_eq!(r.stdout, "12\n");
    }

    #[test]
    fn expr_division() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["10".into(), "/".into(), "3".into()], &c);
        assert_eq!(r.stdout, "3\n");
    }

    #[test]
    fn expr_modulo() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["10".into(), "%".into(), "3".into()], &c);
        assert_eq!(r.stdout, "1\n");
    }

    #[test]
    fn expr_comparison() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["5".into(), ">".into(), "3".into()], &c);
        assert_eq!(r.stdout, "1\n");
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn expr_length() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["length".into(), "hello".into()], &c);
        assert_eq!(r.stdout, "5\n");
    }

    #[test]
    fn expr_substr() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(
            &["substr".into(), "hello".into(), "2".into(), "3".into()],
            &c,
        );
        assert_eq!(r.stdout, "ell\n");
    }

    #[test]
    fn expr_match() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["hello".into(), ":".into(), "hel".into()], &c);
        assert_eq!(r.stdout, "3\n");
    }

    #[test]
    fn expr_division_by_zero() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["5".into(), "/".into(), "0".into()], &c);
        assert_eq!(r.exit_code, 2);
    }

    #[test]
    fn expr_missing_operand() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 2);
    }

    #[test]
    fn expr_zero_result() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExprCommand.execute(&["0".into(), "+".into(), "0".into()], &c);
        assert_eq!(r.stdout, "0\n");
        assert_eq!(r.exit_code, 1);
    }

    // ── date tests ───────────────────────────────────────────────────

    #[test]
    fn date_default() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = DateCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert!(!r.stdout.is_empty());
    }

    #[test]
    fn date_format() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = DateCommand.execute(&["+%Y".into()], &c);
        assert_eq!(r.exit_code, 0);
        let year = r.stdout.trim();
        assert!(year.len() == 4);
        assert!(year.parse::<u32>().is_ok());
    }

    #[test]
    fn date_epoch() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = DateCommand.execute(&["+%s".into()], &c);
        let epoch = r.stdout.trim().parse::<u64>();
        assert!(epoch.is_ok());
        assert!(epoch.unwrap() > 1_000_000_000);
    }

    // ── sleep tests ──────────────────────────────────────────────────

    #[test]
    fn sleep_missing_arg() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SleepCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn sleep_invalid_arg() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SleepCommand.execute(&["abc".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn sleep_zero() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SleepCommand.execute(&["0".into()], &c);
        assert_eq!(r.exit_code, 0);
    }

    // ── seq tests ────────────────────────────────────────────────────

    #[test]
    fn seq_single() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SeqCommand.execute(&["5".into()], &c);
        assert_eq!(r.stdout, "1\n2\n3\n4\n5\n");
    }

    #[test]
    fn seq_range() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SeqCommand.execute(&["3".into(), "6".into()], &c);
        assert_eq!(r.stdout, "3\n4\n5\n6\n");
    }

    #[test]
    fn seq_with_increment() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SeqCommand.execute(&["1".into(), "2".into(), "9".into()], &c);
        assert_eq!(r.stdout, "1\n3\n5\n7\n9\n");
    }

    #[test]
    fn seq_empty() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SeqCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── env tests ────────────────────────────────────────────────────

    #[test]
    fn env_lists_all() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = EnvCommand.execute(&[], &c);
        assert!(r.stdout.contains("USER=testuser"));
        assert!(r.stdout.contains("HOSTNAME=myhost"));
    }

    // ── printenv tests ───────────────────────────────────────────────

    #[test]
    fn printenv_specific() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintenvCommand.execute(&["USER".into()], &c);
        assert_eq!(r.stdout, "testuser\n");
    }

    #[test]
    fn printenv_missing() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintenvCommand.execute(&["NOPE".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn printenv_all() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintenvCommand.execute(&[], &c);
        assert!(r.stdout.contains("USER=testuser"));
    }

    // ── which tests ──────────────────────────────────────────────────

    #[test]
    fn which_builtin() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WhichCommand.execute(&["cd".into()], &c);
        assert!(r.stdout.contains("shell built-in"));
    }

    #[test]
    fn which_registered() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WhichCommand.execute(&["echo".into()], &c);
        assert!(r.stdout.contains("/usr/bin/echo"));
    }

    #[test]
    fn which_not_found() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WhichCommand.execute(&["nonexistent_cmd".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn which_no_args() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WhichCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── base64 tests ─────────────────────────────────────────────────

    #[test]
    fn base64_encode_stdin() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello");
        let r = Base64Command.execute(&[], &c);
        assert_eq!(r.stdout.trim(), "aGVsbG8=");
    }

    #[test]
    fn base64_decode() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "aGVsbG8=");
        let r = Base64Command.execute(&["-d".into()], &c);
        assert_eq!(r.stdout, "hello");
    }

    #[test]
    fn base64_encode_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.bin"), b"test").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = Base64Command.execute(&["test.bin".into()], &c);
        assert_eq!(r.stdout.trim(), "dGVzdA==");
    }

    // ── md5sum tests ─────────────────────────────────────────────────

    #[test]
    fn md5sum_stdin() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello");
        let r = Md5sumCommand.execute(&[], &c);
        assert!(r.stdout.starts_with("5d41402abc4b2a76b9719d911017c592"));
    }

    #[test]
    fn md5sum_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = Md5sumCommand.execute(&["hello.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("hello.txt"));
    }

    #[test]
    fn md5sum_nonexistent() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = Md5sumCommand.execute(&["nope.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── sha256sum tests ──────────────────────────────────────────────

    #[test]
    fn sha256sum_stdin() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello");
        let r = Sha256sumCommand.execute(&[], &c);
        assert!(
            r.stdout
                .starts_with("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
    }

    #[test]
    fn sha256sum_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = Sha256sumCommand.execute(&["hello.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("hello.txt"));
    }

    // ── whoami tests ─────────────────────────────────────────────────

    #[test]
    fn whoami_from_env() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WhoamiCommand.execute(&[], &c);
        assert_eq!(r.stdout, "testuser\n");
    }

    #[test]
    fn whoami_default_root() {
        let (fs, _env, limits, np) = setup();
        let empty_env = HashMap::new();
        let c = ctx(&*fs, &empty_env, &limits, &np);
        let r = WhoamiCommand.execute(&[], &c);
        assert_eq!(r.stdout, "root\n");
    }

    // ── hostname tests ───────────────────────────────────────────────

    #[test]
    fn hostname_from_env() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = HostnameCommand.execute(&[], &c);
        assert_eq!(r.stdout, "myhost\n");
    }

    #[test]
    fn hostname_default() {
        let (fs, _env, limits, np) = setup();
        let empty_env = HashMap::new();
        let c = ctx(&*fs, &empty_env, &limits, &np);
        let r = HostnameCommand.execute(&[], &c);
        assert_eq!(r.stdout, "localhost\n");
    }

    // ── uname tests ──────────────────────────────────────────────────

    #[test]
    fn uname_default() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = UnameCommand.execute(&[], &c);
        assert_eq!(r.stdout, "Linux\n");
    }

    #[test]
    fn uname_all() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = UnameCommand.execute(&["-a".into()], &c);
        assert!(r.stdout.contains("Linux"));
        assert!(r.stdout.contains("rust-bash"));
        assert!(r.stdout.contains("x86_64"));
    }

    #[test]
    fn uname_machine() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = UnameCommand.execute(&["-m".into()], &c);
        assert_eq!(r.stdout, "x86_64\n");
    }

    // ── yes tests ────────────────────────────────────────────────────

    #[test]
    fn yes_default() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = YesCommand.execute(&[], &c);
        let lines: Vec<&str> = r.stdout.lines().collect();
        assert_eq!(lines.len(), 10_000);
        assert!(lines.iter().all(|l| *l == "y"));
    }

    #[test]
    fn yes_custom_string() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = YesCommand.execute(&["hello".into()], &c);
        let lines: Vec<&str> = r.stdout.lines().collect();
        assert_eq!(lines.len(), 10_000);
        assert!(lines.iter().all(|l| *l == "hello"));
    }
}
