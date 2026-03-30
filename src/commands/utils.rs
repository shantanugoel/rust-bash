//! Utility commands: expr, date, sleep, seq, env, printenv, which, base64,
//! md5sum, sha256sum, whoami, hostname, uname, yes

use super::CommandMeta;
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

static EXPR_META: CommandMeta = CommandMeta {
    name: "expr",
    synopsis: "expr EXPRESSION",
    description: "Evaluate expressions.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for ExprCommand {
    fn name(&self) -> &str {
        "expr"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&EXPR_META)
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

static DATE_META: CommandMeta = CommandMeta {
    name: "date",
    synopsis: "date [+FORMAT]",
    description: "Display the current date and time.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for DateCommand {
    fn name(&self) -> &str {
        "date"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&DATE_META)
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

static SLEEP_META: CommandMeta = CommandMeta {
    name: "sleep",
    synopsis: "sleep SECONDS",
    description: "Delay for a specified amount of time.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for SleepCommand {
    fn name(&self) -> &str {
        "sleep"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&SLEEP_META)
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

static SEQ_META: CommandMeta = CommandMeta {
    name: "seq",
    synopsis: "seq [FIRST [INCREMENT]] LAST",
    description: "Print a sequence of numbers.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for SeqCommand {
    fn name(&self) -> &str {
        "seq"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&SEQ_META)
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

static ENV_META: CommandMeta = CommandMeta {
    name: "env",
    synopsis: "env",
    description: "Print the current environment.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for EnvCommand {
    fn name(&self) -> &str {
        "env"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&ENV_META)
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

static PRINTENV_META: CommandMeta = CommandMeta {
    name: "printenv",
    synopsis: "printenv [VARIABLE ...]",
    description: "Print all or part of environment.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for PrintenvCommand {
    fn name(&self) -> &str {
        "printenv"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&PRINTENV_META)
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

static WHICH_META: CommandMeta = CommandMeta {
    name: "which",
    synopsis: "which COMMAND ...",
    description: "Locate a command.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for WhichCommand {
    fn name(&self) -> &str {
        "which"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&WHICH_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        if args.is_empty() {
            return CommandResult {
                stderr: "which: missing argument\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut exit_code = 0;

        let path_dirs: Vec<&str> = ctx
            .env
            .get("PATH")
            .map(|p| p.split(':').collect())
            .unwrap_or_default();

        for arg in args {
            if crate::interpreter::builtins::is_builtin(arg) {
                stdout.push_str(&format!("{arg}: shell built-in command\n"));
            } else {
                let mut found = false;
                for dir in &path_dirs {
                    let full = if dir.is_empty() {
                        format!("./{arg}")
                    } else {
                        format!("{dir}/{arg}")
                    };
                    let p = std::path::Path::new(&full);
                    if ctx.fs.exists(p)
                        && ctx
                            .fs
                            .stat(p)
                            .is_ok_and(|m| m.node_type != crate::vfs::NodeType::Directory)
                    {
                        stdout.push_str(&full);
                        stdout.push('\n');
                        found = true;
                        break;
                    }
                }
                if !found {
                    exit_code = 1;
                }
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

static BASE64_META: CommandMeta = CommandMeta {
    name: "base64",
    synopsis: "base64 [-d] [-w COLS] [FILE]",
    description: "Base64 encode or decode data.",
    options: &[
        ("-d, --decode", "decode data"),
        ("-w COLS", "wrap encoded lines after COLS characters"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for Base64Command {
    fn name(&self) -> &str {
        "base64"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&BASE64_META)
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

static MD5SUM_META: CommandMeta = CommandMeta {
    name: "md5sum",
    synopsis: "md5sum [FILE ...]",
    description: "Compute and check MD5 message digest.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for Md5sumCommand {
    fn name(&self) -> &str {
        "md5sum"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&MD5SUM_META)
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

static SHA256SUM_META: CommandMeta = CommandMeta {
    name: "sha256sum",
    synopsis: "sha256sum [FILE ...]",
    description: "Compute and check SHA256 message digest.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for Sha256sumCommand {
    fn name(&self) -> &str {
        "sha256sum"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&SHA256SUM_META)
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

static WHOAMI_META: CommandMeta = CommandMeta {
    name: "whoami",
    synopsis: "whoami",
    description: "Print effective user name.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for WhoamiCommand {
    fn name(&self) -> &str {
        "whoami"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&WHOAMI_META)
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

static HOSTNAME_META: CommandMeta = CommandMeta {
    name: "hostname",
    synopsis: "hostname",
    description: "Show the system's host name.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for HostnameCommand {
    fn name(&self) -> &str {
        "hostname"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&HOSTNAME_META)
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

static UNAME_META: CommandMeta = CommandMeta {
    name: "uname",
    synopsis: "uname [-amnrs]",
    description: "Print system information.",
    options: &[
        ("-a", "print all information"),
        ("-s", "print the kernel name"),
        ("-n", "print the network node hostname"),
        ("-r", "print the kernel release"),
        ("-m", "print the machine hardware name"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for UnameCommand {
    fn name(&self) -> &str {
        "uname"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&UNAME_META)
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

static YES_META: CommandMeta = CommandMeta {
    name: "yes",
    synopsis: "yes [STRING]",
    description: "Output a string repeatedly until killed.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for YesCommand {
    fn name(&self) -> &str {
        "yes"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&YES_META)
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

// ── sha1sum ──────────────────────────────────────────────────────────

pub struct Sha1sumCommand;

static SHA1SUM_META: CommandMeta = CommandMeta {
    name: "sha1sum",
    synopsis: "sha1sum [FILE ...]",
    description: "Compute and check SHA1 message digest.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for Sha1sumCommand {
    fn name(&self) -> &str {
        "sha1sum"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&SHA1SUM_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        use sha1::Digest;

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
                        stderr.push_str(&format!("sha1sum: {}: {}\n", file, e));
                        exit_code = 1;
                        continue;
                    }
                }
            };

            let mut hasher = sha1::Sha1::new();
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

// ── timeout ─────────────────────────────────────────────────────────

pub struct TimeoutCommand;

static TIMEOUT_META: CommandMeta = CommandMeta {
    name: "timeout",
    synopsis: "timeout [-k DURATION] [-s SIGNAL] DURATION COMMAND [ARG...]",
    description: "Run a command with a time limit.",
    options: &[
        (
            "-k DURATION",
            "send a kill signal after DURATION (no-op in sandbox)",
        ),
        ("-s SIGNAL", "specify the signal to send (no-op in sandbox)"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for TimeoutCommand {
    fn name(&self) -> &str {
        "timeout"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TIMEOUT_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut i = 0;
        // Skip optional flags -k and -s (no-op)
        while i < args.len() {
            let arg = &args[i];
            if arg == "-k" || arg == "--kill-after" {
                i += 2; // skip flag + value
            } else if arg == "-s" || arg == "--signal" {
                i += 2;
            } else if arg.starts_with("--kill-after=") || arg.starts_with("--signal=") {
                i += 1;
            } else {
                break;
            }
        }

        if i >= args.len() {
            return CommandResult {
                stderr: "timeout: missing operand\n".into(),
                exit_code: 125,
                ..Default::default()
            };
        }

        let duration_str = &args[i];
        i += 1;

        let duration_secs: f64 = match duration_str.parse() {
            Ok(d) => d,
            Err(_) => {
                return CommandResult {
                    stderr: format!("timeout: invalid time interval '{}'\n", duration_str),
                    exit_code: 125,
                    ..Default::default()
                };
            }
        };

        if i >= args.len() {
            return CommandResult {
                stderr: "timeout: missing operand\n".into(),
                exit_code: 125,
                ..Default::default()
            };
        }

        let exec = match ctx.exec {
            Some(cb) => cb,
            None => {
                return CommandResult {
                    stderr: "timeout: exec callback not available\n".into(),
                    exit_code: 126,
                    ..Default::default()
                };
            }
        };

        let cmd_line = args[i..].join(" ");
        let start = crate::platform::Instant::now();

        // NOTE: In this sandboxed, single-threaded interpreter there is no
        // signal mechanism to preemptively kill the child.  We run the
        // command synchronously and check elapsed time *after* it finishes,
        // so a long-running command will block for its full duration.  The
        // global `max_execution_time` limit may still interrupt, but with
        // its own error rather than exit code 124.
        match exec(&cmd_line) {
            Ok(result) => {
                let elapsed = start.elapsed();
                if elapsed.as_secs_f64() > duration_secs {
                    CommandResult {
                        stdout: result.stdout,
                        stderr: result.stderr,
                        exit_code: 124,
                    }
                } else {
                    result
                }
            }
            Err(e) => {
                let elapsed = start.elapsed();
                if elapsed.as_secs_f64() > duration_secs {
                    CommandResult {
                        stderr: format!("{}\n", e),
                        exit_code: 124,
                        ..Default::default()
                    }
                } else {
                    CommandResult {
                        stderr: format!("timeout: {}\n", e),
                        exit_code: 126,
                        ..Default::default()
                    }
                }
            }
        }
    }
}

// ── file ─────────────────────────────────────────────────────────────

pub struct FileCommand;

static FILE_META: CommandMeta = CommandMeta {
    name: "file",
    synopsis: "file FILE...",
    description: "Determine file type.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for FileCommand {
    fn name(&self) -> &str {
        "file"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&FILE_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        if args.is_empty() {
            return CommandResult {
                stderr: "file: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for arg in args {
            if arg == "--" {
                continue;
            }
            let path = resolve_path(arg, ctx.cwd);
            let meta = match ctx.fs.stat(&path) {
                Ok(m) => m,
                Err(e) => {
                    stderr.push_str(&format!("{}: cannot open ({})\n", arg, e));
                    exit_code = 1;
                    continue;
                }
            };

            use crate::vfs::NodeType;
            let file_type = match meta.node_type {
                NodeType::Directory => "directory".to_string(),
                NodeType::Symlink => "symbolic link".to_string(),
                NodeType::File => match ctx.fs.read_file(&path) {
                    Ok(data) => detect_file_type(&data, arg),
                    Err(_) => "regular file".to_string(),
                },
            };

            stdout.push_str(&format!("{}: {}\n", arg, file_type));
        }

        CommandResult {
            stdout,
            stderr,
            exit_code,
        }
    }
}

fn detect_file_type(data: &[u8], name: &str) -> String {
    if data.is_empty() {
        return "empty".to_string();
    }

    // Magic-byte detection
    if data.len() >= 8 && &data[0..8] == b"\x89PNG\r\n\x1a\n" {
        return "PNG image data".to_string();
    }
    if data.len() >= 3 && &data[0..3] == b"\xff\xd8\xff" {
        return "JPEG image data".to_string();
    }
    if data.len() >= 6 && (&data[0..6] == b"GIF87a" || &data[0..6] == b"GIF89a") {
        return "GIF image data".to_string();
    }
    if data.len() >= 4 && &data[0..4] == b"\x7fELF" {
        return "ELF executable".to_string();
    }
    if data.len() >= 2 && &data[0..2] == b"\x1f\x8b" {
        return "gzip compressed data".to_string();
    }
    if data.len() >= 5 && &data[0..5] == b"%PDF-" {
        return "PDF document".to_string();
    }
    if data.len() >= 4 && &data[0..4] == b"PK\x03\x04" {
        return "Zip archive data".to_string();
    }
    if data.len() >= 263 && &data[257..262] == b"ustar" {
        return "POSIX tar archive".to_string();
    }

    // Check if it looks like text
    let sample = &data[..data.len().min(512)];
    let is_text = sample
        .iter()
        .all(|&b| b == b'\n' || b == b'\r' || b == b'\t' || (0x20..0x7f).contains(&b));

    if is_text {
        // Check for JSON
        let text = String::from_utf8_lossy(sample);
        let trimmed = text.trim();
        if (trimmed.starts_with('{') && trimmed.ends_with('}'))
            || (trimmed.starts_with('[') && trimmed.ends_with(']'))
        {
            return "JSON text data".to_string();
        }
        // Check for XML
        if trimmed.starts_with("<?xml") {
            return "XML document".to_string();
        }

        // Extension fallback
        let ext = name.rsplit('.').next().unwrap_or("");
        match ext {
            "sh" | "bash" => return "Bourne-Again shell script, ASCII text".to_string(),
            "py" => return "Python script, ASCII text".to_string(),
            "rb" => return "Ruby script, ASCII text".to_string(),
            "js" => return "JavaScript source, ASCII text".to_string(),
            "ts" => return "TypeScript source, ASCII text".to_string(),
            "rs" => return "Rust source, ASCII text".to_string(),
            "c" => return "C source, ASCII text".to_string(),
            "h" => return "C header, ASCII text".to_string(),
            "cpp" | "cc" | "cxx" => return "C++ source, ASCII text".to_string(),
            "java" => return "Java source, ASCII text".to_string(),
            "go" => return "Go source, ASCII text".to_string(),
            "pl" => return "Perl script, ASCII text".to_string(),
            "html" | "htm" => return "HTML document, ASCII text".to_string(),
            "css" => return "CSS source, ASCII text".to_string(),
            "json" => return "JSON text data".to_string(),
            "xml" => return "XML document".to_string(),
            "yaml" | "yml" => return "YAML document, ASCII text".to_string(),
            "toml" => return "TOML document, ASCII text".to_string(),
            "md" => return "Markdown document, ASCII text".to_string(),
            "txt" => return "ASCII text".to_string(),
            _ => {}
        }

        return "ASCII text".to_string();
    }

    "data".to_string()
}

// ── bc ──────────────────────────────────────────────────────────────

pub struct BcCommand;

static BC_META: CommandMeta = CommandMeta {
    name: "bc",
    synopsis: "bc [-l] [file ...]",
    description: "An arbitrary precision calculator language.",
    options: &[("-l", "use the standard math library (set scale=20)")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for BcCommand {
    fn name(&self) -> &str {
        "bc"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&BC_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut scale: u32 = 0;
        let mut files: Vec<&str> = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-l" => scale = 20,
                _ => files.push(arg),
            }
        }

        let input = if !files.is_empty() {
            let mut combined = String::new();
            for f in &files {
                let path = resolve_path(f, ctx.cwd);
                match ctx.fs.read_file(&path) {
                    Ok(bytes) => {
                        combined.push_str(&String::from_utf8_lossy(&bytes));
                        combined.push('\n');
                    }
                    Err(e) => {
                        return CommandResult {
                            stderr: format!("bc: {}: {}\n", f, e),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                }
            }
            combined
        } else {
            ctx.stdin.to_string()
        };

        let mut env = BcEnv {
            scale,
            vars: std::collections::HashMap::new(),
        };

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for raw_line in input.lines() {
            // Split on semicolons for multi-statement lines
            for stmt in raw_line.split(';') {
                let stmt = stmt.trim();
                if stmt.is_empty() || stmt == "quit" {
                    continue;
                }

                // Check for scale assignment
                if let Some(val_str) = stmt.strip_prefix("scale") {
                    let val_str = val_str.trim();
                    if let Some(val_str) = val_str.strip_prefix('=') {
                        let val_str = val_str.trim();
                        match val_str.parse::<u32>() {
                            Ok(v) => {
                                env.scale = v;
                                continue;
                            }
                            Err(_) => {
                                stderr.push_str(&format!("bc: parse error: {}\n", stmt));
                                exit_code = 1;
                                continue;
                            }
                        }
                    }
                }

                // Check for variable assignment (simple: name = expr)
                if let Some(eq_pos) = stmt.find('=') {
                    let lhs = stmt[..eq_pos].trim();
                    let rhs = stmt[eq_pos + 1..].trim();
                    // Make sure LHS is identifier and not comparison
                    if !lhs.is_empty()
                        && lhs.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                        && lhs
                            .chars()
                            .next()
                            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                        && !rhs.starts_with('=')
                        && !stmt[..eq_pos].ends_with('!')
                        && !stmt[..eq_pos].ends_with('<')
                        && !stmt[..eq_pos].ends_with('>')
                    {
                        match bc_parse_expr(&mut BcParser::new(rhs), &env, 0) {
                            Ok(val) => {
                                env.vars.insert(lhs.to_string(), val);
                                continue;
                            }
                            Err(e) => {
                                stderr.push_str(&format!("bc: {}\n", e));
                                exit_code = 1;
                                continue;
                            }
                        }
                    }
                }

                // Expression evaluation
                match bc_parse_expr(&mut BcParser::new(stmt), &env, 0) {
                    Ok(val) => {
                        stdout.push_str(&bc_format_number(val, env.scale));
                        stdout.push('\n');
                    }
                    Err(e) => {
                        stderr.push_str(&format!("bc: {}\n", e));
                        exit_code = 1;
                    }
                }
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code,
        }
    }
}

struct BcEnv {
    scale: u32,
    vars: std::collections::HashMap<String, f64>,
}

struct BcParser<'a> {
    input: &'a str,
    pos: usize,
}

impl<'a> BcParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.input.len() && self.input.as_bytes()[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> Option<char> {
        self.skip_ws();
        self.input[self.pos..].chars().next()
    }

    fn peek_two(&mut self) -> Option<&'a str> {
        self.skip_ws();
        if self.pos + 1 < self.input.len() {
            Some(&self.input[self.pos..self.pos + 2])
        } else {
            None
        }
    }

    fn advance(&mut self) {
        if self.pos < self.input.len() {
            self.pos += self.input[self.pos..]
                .chars()
                .next()
                .map_or(0, |c| c.len_utf8());
        }
    }

    fn at_end(&mut self) -> bool {
        self.skip_ws();
        self.pos >= self.input.len()
    }
}

fn bc_parse_expr(parser: &mut BcParser, env: &BcEnv, min_prec: u8) -> Result<f64, String> {
    let mut left = bc_parse_unary(parser, env)?;

    loop {
        if parser.at_end() {
            break;
        }

        let (op, prec, right_assoc) = match parser.peek_two() {
            Some("==") => ("==", 1, false),
            Some("!=") => ("!=", 1, false),
            Some("<=") => ("<=", 2, false),
            Some(">=") => (">=", 2, false),
            _ => match parser.peek() {
                Some('<') => ("<", 2, false),
                Some('>') => (">", 2, false),
                Some('+') => ("+", 3, false),
                Some('-') => ("-", 3, false),
                Some('*') => ("*", 4, false),
                Some('/') => ("/", 4, false),
                Some('%') => ("%", 4, false),
                Some('^') => ("^", 5, true),
                _ => break,
            },
        };

        if prec < min_prec {
            break;
        }

        // Consume operator
        for _ in 0..op.len() {
            parser.advance();
        }

        let next_min = if right_assoc { prec } else { prec + 1 };
        let right = bc_parse_expr(parser, env, next_min)?;

        left = match op {
            "+" => left + right,
            "-" => left - right,
            "*" => left * right,
            "/" => {
                if right == 0.0 {
                    return Err("divide by zero".to_string());
                }
                left / right
            }
            "%" => {
                if right == 0.0 {
                    return Err("divide by zero".to_string());
                }
                left % right
            }
            "^" => left.powf(right),
            "==" => {
                if (left - right).abs() < f64::EPSILON {
                    1.0
                } else {
                    0.0
                }
            }
            "!=" => {
                if (left - right).abs() >= f64::EPSILON {
                    1.0
                } else {
                    0.0
                }
            }
            "<" => {
                if left < right {
                    1.0
                } else {
                    0.0
                }
            }
            ">" => {
                if left > right {
                    1.0
                } else {
                    0.0
                }
            }
            "<=" => {
                if left <= right {
                    1.0
                } else {
                    0.0
                }
            }
            ">=" => {
                if left >= right {
                    1.0
                } else {
                    0.0
                }
            }
            _ => unreachable!(),
        };
    }

    Ok(left)
}

fn bc_parse_unary(parser: &mut BcParser, env: &BcEnv) -> Result<f64, String> {
    match parser.peek() {
        Some('-') => {
            parser.advance();
            let val = bc_parse_unary(parser, env)?;
            Ok(-val)
        }
        Some('+') => {
            parser.advance();
            bc_parse_unary(parser, env)
        }
        _ => bc_parse_primary(parser, env),
    }
}

fn bc_parse_primary(parser: &mut BcParser, env: &BcEnv) -> Result<f64, String> {
    parser.skip_ws();

    if parser.peek() == Some('(') {
        parser.advance();
        let val = bc_parse_expr(parser, env, 0)?;
        if parser.peek() != Some(')') {
            return Err("expected ')'".to_string());
        }
        parser.advance();
        return Ok(val);
    }

    // Number
    let start = parser.pos;
    let input = parser.input;
    while parser.pos < input.len() {
        let ch = input.as_bytes()[parser.pos];
        if ch.is_ascii_digit() || ch == b'.' {
            parser.pos += 1;
        } else {
            break;
        }
    }

    if parser.pos > start {
        let num_str = &input[start..parser.pos];
        return num_str
            .parse::<f64>()
            .map_err(|_| format!("invalid number: {}", num_str));
    }

    // Variable name
    let var_start = parser.pos;
    while parser.pos < input.len() {
        let ch = input.as_bytes()[parser.pos];
        if ch.is_ascii_alphanumeric() || ch == b'_' {
            parser.pos += 1;
        } else {
            break;
        }
    }

    if parser.pos > var_start {
        let name = &input[var_start..parser.pos];
        if name == "scale" {
            return Ok(env.scale as f64);
        }
        return Ok(*env.vars.get(name).unwrap_or(&0.0));
    }

    Err(format!("parse error at position {}", parser.pos))
}

fn bc_format_number(val: f64, scale: u32) -> String {
    if scale == 0 {
        // Truncate towards zero
        let truncated = val as i64;
        return truncated.to_string();
    }

    let formatted = format!("{:.prec$}", val, prec = scale as usize);
    // Remove trailing zeros after decimal, but keep at least `scale` digits? No, bc keeps them.
    formatted
}

// ── clear ───────────────────────────────────────────────────────────

pub struct ClearCommand;

static CLEAR_META: CommandMeta = CommandMeta {
    name: "clear",
    synopsis: "clear",
    description: "Clear the terminal screen.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for ClearCommand {
    fn name(&self) -> &str {
        "clear"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&CLEAR_META)
    }

    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: "\x1b[2J\x1b[H".to_string(),
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
        let (fs, mut env, limits, np) = setup();
        env.insert("PATH".into(), "/usr/bin:/bin".into());
        fs.mkdir_p(Path::new("/bin")).unwrap();
        fs.write_file(Path::new("/bin/echo"), b"#!/bin/bash\n# built-in: echo\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WhichCommand.execute(&["echo".into()], &c);
        assert!(r.stdout.contains("/bin/echo"));
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

    #[test]
    fn which_multi_args_mixed() {
        let (fs, mut env, limits, np) = setup();
        env.insert("PATH".into(), "/bin".into());
        fs.mkdir_p(Path::new("/bin")).unwrap();
        fs.write_file(Path::new("/bin/echo"), b"#!/bin/bash\n# built-in: echo\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WhichCommand.execute(&["cd".into(), "echo".into(), "nonexistent".into()], &c);
        assert!(r.stdout.contains("shell built-in"));
        assert!(r.stdout.contains("/bin/echo"));
        assert_eq!(r.exit_code, 1); // at least one not found
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
