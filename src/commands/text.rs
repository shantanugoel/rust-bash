//! Text processing commands: grep, sort, uniq, cut, head, tail, wc, tr, rev, fold, nl, printf, paste

use crate::commands::{CommandContext, CommandResult};
use regex::Regex;
use std::path::PathBuf;

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

fn read_input(
    files: &[&str],
    ctx: &CommandContext,
) -> Result<Vec<(String, String)>, CommandResult> {
    let mut result = Vec::new();
    if files.is_empty() {
        result.push(("(standard input)".to_string(), ctx.stdin.to_string()));
        return Ok(result);
    }
    for file in files {
        if *file == "-" {
            result.push(("(standard input)".to_string(), ctx.stdin.to_string()));
        } else {
            let path = resolve_path(file, ctx.cwd);
            match ctx.fs.read_file(&path) {
                Ok(bytes) => {
                    result.push((
                        file.to_string(),
                        String::from_utf8_lossy(&bytes).to_string(),
                    ));
                }
                Err(e) => {
                    return Err(CommandResult {
                        stderr: format!("{}: {}\n", file, e),
                        exit_code: 1,
                        ..Default::default()
                    });
                }
            }
        }
    }
    Ok(result)
}

/// Read all named files (or stdin) and concatenate their content.
fn read_all_input(files: &[&str], ctx: &CommandContext) -> (String, String, i32) {
    let mut content = String::new();
    let mut stderr = String::new();
    let mut exit_code = 0;

    if files.is_empty() {
        content.push_str(ctx.stdin);
    } else {
        for file in files {
            if *file == "-" {
                content.push_str(ctx.stdin);
            } else {
                let path = resolve_path(file, ctx.cwd);
                match ctx.fs.read_file(&path) {
                    Ok(bytes) => {
                        content.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    Err(e) => {
                        stderr.push_str(&format!("{}: {}\n", file, e));
                        exit_code = 1;
                    }
                }
            }
        }
    }
    (content, stderr, exit_code)
}

// ── grep ─────────────────────────────────────────────────────────────

pub struct GrepCommand;

impl super::VirtualCommand for GrepCommand {
    fn name(&self) -> &str {
        "grep"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut case_insensitive = false;
        let mut invert = false;
        let mut show_line_numbers = false;
        let mut count_only = false;
        let mut files_with_matches = false;
        let mut fixed_string = false;
        let mut opts_done = false;
        let mut pattern_str: Option<&str> = None;
        let mut files: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    match c {
                        'i' => case_insensitive = true,
                        'v' => invert = true,
                        'n' => show_line_numbers = true,
                        'c' => count_only = true,
                        'l' => files_with_matches = true,
                        'F' => fixed_string = true,
                        _ => {}
                    }
                }
            } else if pattern_str.is_none() {
                pattern_str = Some(arg);
            } else {
                files.push(arg);
            }
        }

        let pattern_str = match pattern_str {
            Some(p) => p,
            None => {
                return CommandResult {
                    stderr: "grep: missing pattern\n".into(),
                    exit_code: 2,
                    ..Default::default()
                };
            }
        };

        let regex_pattern = if fixed_string {
            regex::escape(pattern_str)
        } else {
            pattern_str.to_string()
        };
        let regex_pattern = if case_insensitive {
            format!("(?i){}", regex_pattern)
        } else {
            regex_pattern
        };

        let re = match Regex::new(&regex_pattern) {
            Ok(r) => r,
            Err(e) => {
                return CommandResult {
                    stderr: format!("grep: invalid regex: {}\n", e),
                    exit_code: 2,
                    ..Default::default()
                };
            }
        };

        let inputs = match read_input(&files, ctx) {
            Ok(i) => i,
            Err(r) => return r,
        };
        let multi_file = inputs.len() > 1;

        let mut stdout = String::new();
        let mut any_match = false;

        for (filename, content) in &inputs {
            let mut file_match_count = 0u64;
            let mut file_matched = false;

            for (line_num, line) in content.lines().enumerate() {
                let matched = re.is_match(line);
                let matched = if invert { !matched } else { matched };

                if matched {
                    file_match_count += 1;
                    file_matched = true;
                    any_match = true;

                    if !count_only && !files_with_matches {
                        if multi_file {
                            stdout.push_str(filename);
                            stdout.push(':');
                        }
                        if show_line_numbers {
                            stdout.push_str(&format!("{}:", line_num + 1));
                        }
                        stdout.push_str(line);
                        stdout.push('\n');
                    }
                }
            }

            if count_only {
                if multi_file {
                    stdout.push_str(&format!("{}:{}\n", filename, file_match_count));
                } else {
                    stdout.push_str(&format!("{}\n", file_match_count));
                }
            }

            if files_with_matches && file_matched {
                stdout.push_str(filename);
                stdout.push('\n');
            }
        }

        CommandResult {
            stdout,
            exit_code: if any_match { 0 } else { 1 },
            ..Default::default()
        }
    }
}

// ── sort ─────────────────────────────────────────────────────────────

pub struct SortCommand;

impl super::VirtualCommand for SortCommand {
    fn name(&self) -> &str {
        "sort"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut reverse = false;
        let mut numeric = false;
        let mut unique = false;
        let mut key_field: Option<usize> = None;
        let mut delimiter: Option<char> = None;
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
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                let mut chars = arg[1..].chars().peekable();
                while let Some(c) = chars.next() {
                    match c {
                        'r' => reverse = true,
                        'n' => numeric = true,
                        'u' => unique = true,
                        'k' => {
                            let rest: String = chars.collect();
                            if !rest.is_empty() {
                                // -k3 style: extract just the field number
                                let field_str = rest
                                    .split(|c: char| !c.is_ascii_digit())
                                    .next()
                                    .unwrap_or("");
                                key_field = field_str.parse().ok();
                            } else {
                                i += 1;
                                if i < args.len() {
                                    let field_str = args[i]
                                        .split(|c: char| !c.is_ascii_digit())
                                        .next()
                                        .unwrap_or("");
                                    key_field = field_str.parse().ok();
                                }
                            }
                            break;
                        }
                        't' => {
                            let rest: String = chars.collect();
                            if !rest.is_empty() {
                                delimiter = rest.chars().next();
                            } else {
                                i += 1;
                                if i < args.len() {
                                    delimiter = args[i].chars().next();
                                }
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            } else {
                files.push(arg);
            }
            i += 1;
        }

        let (content, stderr, err_code) = read_all_input(&files, ctx);
        if err_code != 0 {
            return CommandResult {
                stderr,
                exit_code: err_code,
                ..Default::default()
            };
        }

        let mut lines: Vec<&str> = content.lines().collect();
        let delim = delimiter.unwrap_or('\t');

        lines.sort_by(|a, b| {
            let a_key = extract_sort_key(a, key_field, delim);
            let b_key = extract_sort_key(b, key_field, delim);

            if numeric {
                let an = a_key.trim().parse::<f64>().unwrap_or(0.0);
                let bn = b_key.trim().parse::<f64>().unwrap_or(0.0);
                an.partial_cmp(&bn).unwrap_or(std::cmp::Ordering::Equal)
            } else {
                a_key.cmp(b_key)
            }
        });

        if reverse {
            lines.reverse();
        }

        if unique {
            lines.dedup();
        }

        let mut stdout = String::new();
        for line in lines {
            stdout.push_str(line);
            stdout.push('\n');
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

fn extract_sort_key(line: &str, key_field: Option<usize>, delim: char) -> &str {
    match key_field {
        Some(k) if k > 0 => line.split(delim).nth(k - 1).unwrap_or(""),
        _ => line,
    }
}

// ── uniq ─────────────────────────────────────────────────────────────

pub struct UniqCommand;

impl super::VirtualCommand for UniqCommand {
    fn name(&self) -> &str {
        "uniq"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut count = false;
        let mut duplicates_only = false;
        let mut unique_only = false;
        let mut opts_done = false;
        let mut files: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    match c {
                        'c' => count = true,
                        'd' => duplicates_only = true,
                        'u' => unique_only = true,
                        _ => {}
                    }
                }
            } else {
                files.push(arg);
            }
        }

        let (content, stderr, err_code) = read_all_input(&files, ctx);
        if err_code != 0 {
            return CommandResult {
                stderr,
                exit_code: err_code,
                ..Default::default()
            };
        }

        let lines: Vec<&str> = content.lines().collect();
        let mut groups: Vec<(usize, &str)> = Vec::new();

        for line in &lines {
            if let Some(last) = groups.last_mut()
                && last.1 == *line
            {
                last.0 += 1;
                continue;
            }
            groups.push((1, line));
        }

        let mut stdout = String::new();
        for (cnt, line) in &groups {
            if duplicates_only && *cnt < 2 {
                continue;
            }
            if unique_only && *cnt > 1 {
                continue;
            }
            if count {
                stdout.push_str(&format!("{:>7} {}\n", cnt, line));
            } else {
                stdout.push_str(line);
                stdout.push('\n');
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

// ── cut ──────────────────────────────────────────────────────────────

pub struct CutCommand;

impl super::VirtualCommand for CutCommand {
    fn name(&self) -> &str {
        "cut"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut delimiter = '\t';
        let mut fields: Option<String> = None;
        let mut chars: Option<String> = None;
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
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                let mut chs = arg[1..].chars().peekable();
                while let Some(c) = chs.next() {
                    match c {
                        'd' => {
                            let rest: String = chs.collect();
                            if !rest.is_empty() {
                                delimiter = rest.chars().next().unwrap_or('\t');
                            } else {
                                i += 1;
                                if i < args.len() {
                                    delimiter = args[i].chars().next().unwrap_or('\t');
                                }
                            }
                            break;
                        }
                        'f' => {
                            let rest: String = chs.collect();
                            if !rest.is_empty() {
                                fields = Some(rest);
                            } else {
                                i += 1;
                                if i < args.len() {
                                    fields = Some(args[i].clone());
                                }
                            }
                            break;
                        }
                        'c' => {
                            let rest: String = chs.collect();
                            if !rest.is_empty() {
                                chars = Some(rest);
                            } else {
                                i += 1;
                                if i < args.len() {
                                    chars = Some(args[i].clone());
                                }
                            }
                            break;
                        }
                        _ => {}
                    }
                }
            } else {
                files.push(arg);
            }
            i += 1;
        }

        if fields.is_none() && chars.is_none() {
            return CommandResult {
                stderr: "cut: you must specify a list of bytes, characters, or fields\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let (content, stderr, err_code) = read_all_input(&files, ctx);
        if err_code != 0 {
            return CommandResult {
                stderr,
                exit_code: err_code,
                ..Default::default()
            };
        }

        let mut stdout = String::new();

        if let Some(ref field_spec) = fields {
            let field_indices = parse_range_spec(field_spec);
            for line in content.lines() {
                let parts: Vec<&str> = line.split(delimiter).collect();
                let mut selected: Vec<&str> = Vec::new();
                for idx in &field_indices {
                    if *idx > 0 && *idx <= parts.len() {
                        selected.push(parts[*idx - 1]);
                    }
                }
                stdout.push_str(&selected.join(&delimiter.to_string()));
                stdout.push('\n');
            }
        } else if let Some(ref char_spec) = chars {
            let char_indices = parse_range_spec(char_spec);
            for line in content.lines() {
                let line_chars: Vec<char> = line.chars().collect();
                let mut selected = String::new();
                for idx in &char_indices {
                    if *idx > 0 && *idx <= line_chars.len() {
                        selected.push(line_chars[*idx - 1]);
                    }
                }
                stdout.push_str(&selected);
                stdout.push('\n');
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

fn parse_range_spec(spec: &str) -> Vec<usize> {
    let mut result = Vec::new();
    for part in spec.split(',') {
        if let Some((start_s, end_s)) = part.split_once('-') {
            let start: usize = start_s.parse().unwrap_or(1);
            let end: usize = end_s.parse().unwrap_or(start);
            for i in start..=end {
                result.push(i);
            }
        } else if let Ok(n) = part.parse::<usize>() {
            result.push(n);
        }
    }
    result
}

// ── head ─────────────────────────────────────────────────────────────

pub struct HeadCommand;

impl super::VirtualCommand for HeadCommand {
    fn name(&self) -> &str {
        "head"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut num_lines: usize = 10;
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
            if !opts_done && arg == "-n" {
                i += 1;
                if i < args.len() {
                    num_lines = args[i].parse().unwrap_or(10);
                }
            } else if !opts_done && arg.starts_with("-n") {
                num_lines = arg[2..].parse().unwrap_or(10);
            } else if !opts_done
                && arg.starts_with('-')
                && arg.len() > 1
                && arg[1..].chars().all(|c| c.is_ascii_digit())
            {
                num_lines = arg[1..].parse().unwrap_or(10);
            } else {
                files.push(arg);
            }
            i += 1;
        }

        let inputs = match read_input(&files, ctx) {
            Ok(i) => i,
            Err(r) => return r,
        };
        let multi = inputs.len() > 1;

        let mut stdout = String::new();
        for (idx, (filename, content)) in inputs.iter().enumerate() {
            if multi {
                if idx > 0 {
                    stdout.push('\n');
                }
                stdout.push_str(&format!("==> {} <==\n", filename));
            }
            for line in content.lines().take(num_lines) {
                stdout.push_str(line);
                stdout.push('\n');
            }
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── tail ─────────────────────────────────────────────────────────────

pub struct TailCommand;

impl super::VirtualCommand for TailCommand {
    fn name(&self) -> &str {
        "tail"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut num_lines: usize = 10;
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
            if !opts_done && arg == "-n" {
                i += 1;
                if i < args.len() {
                    let val = &args[i];
                    if let Some(stripped) = val.strip_prefix('+') {
                        // +N means starting from line N (handled below)
                        num_lines = stripped.parse().unwrap_or(10);
                        // We use a sentinel to distinguish +N vs N
                        // For simplicity, just handle -n N (last N lines)
                    } else {
                        num_lines = val.parse().unwrap_or(10);
                    }
                }
            } else if !opts_done && arg.starts_with("-n") {
                num_lines = arg[2..].parse().unwrap_or(10);
            } else if !opts_done
                && arg.starts_with('-')
                && arg.len() > 1
                && arg[1..].chars().all(|c| c.is_ascii_digit())
            {
                num_lines = arg[1..].parse().unwrap_or(10);
            } else {
                files.push(arg);
            }
            i += 1;
        }

        let inputs = match read_input(&files, ctx) {
            Ok(i) => i,
            Err(r) => return r,
        };
        let multi = inputs.len() > 1;

        let mut stdout = String::new();
        for (idx, (filename, content)) in inputs.iter().enumerate() {
            if multi {
                if idx > 0 {
                    stdout.push('\n');
                }
                stdout.push_str(&format!("==> {} <==\n", filename));
            }
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(num_lines);
            for line in &lines[start..] {
                stdout.push_str(line);
                stdout.push('\n');
            }
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── wc ───────────────────────────────────────────────────────────────

pub struct WcCommand;

impl super::VirtualCommand for WcCommand {
    fn name(&self) -> &str {
        "wc"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut show_lines = false;
        let mut show_words = false;
        let mut show_bytes = false;
        let mut opts_done = false;
        let mut files: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    match c {
                        'l' => show_lines = true,
                        'w' => show_words = true,
                        'c' => show_bytes = true,
                        _ => {}
                    }
                }
            } else {
                files.push(arg);
            }
        }

        // Default: show all three
        if !show_lines && !show_words && !show_bytes {
            show_lines = true;
            show_words = true;
            show_bytes = true;
        }

        let inputs = match read_input(&files, ctx) {
            Ok(i) => i,
            Err(r) => return r,
        };

        let mut stdout = String::new();
        let mut total_lines = 0usize;
        let mut total_words = 0usize;
        let mut total_bytes = 0usize;

        for (filename, content) in &inputs {
            let line_count = content.lines().count();
            // If content ends with newline, that's accurate; if not, lines() still counts last
            let word_count = content.split_whitespace().count();
            let byte_count = content.len();

            total_lines += line_count;
            total_words += word_count;
            total_bytes += byte_count;

            let mut parts = Vec::new();
            if show_lines {
                parts.push(format!("{:>7}", line_count));
            }
            if show_words {
                parts.push(format!("{:>7}", word_count));
            }
            if show_bytes {
                parts.push(format!("{:>7}", byte_count));
            }

            let display_name = if files.is_empty() {
                String::new()
            } else {
                format!(" {}", filename)
            };
            stdout.push_str(&format!("{}{}\n", parts.join(""), display_name));
        }

        if inputs.len() > 1 {
            let mut parts = Vec::new();
            if show_lines {
                parts.push(format!("{:>7}", total_lines));
            }
            if show_words {
                parts.push(format!("{:>7}", total_words));
            }
            if show_bytes {
                parts.push(format!("{:>7}", total_bytes));
            }
            stdout.push_str(&format!("{} total\n", parts.join("")));
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── tr ───────────────────────────────────────────────────────────────

pub struct TrCommand;

impl super::VirtualCommand for TrCommand {
    fn name(&self) -> &str {
        "tr"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut delete = false;
        let mut squeeze = false;
        let mut opts_done = false;
        let mut operands: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    match c {
                        'd' => delete = true,
                        's' => squeeze = true,
                        _ => {}
                    }
                }
            } else {
                operands.push(arg);
            }
        }

        if operands.is_empty() {
            return CommandResult {
                stderr: "tr: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let set1 = expand_tr_set(operands[0]);
        let set2 = if operands.len() > 1 {
            expand_tr_set(operands[1])
        } else {
            Vec::new()
        };

        let input = ctx.stdin;
        let mut result = String::with_capacity(input.len());

        if delete {
            for c in input.chars() {
                if !set1.contains(&c) {
                    result.push(c);
                }
            }
        } else if squeeze && set2.is_empty() {
            let mut last_char: Option<char> = None;
            for c in input.chars() {
                if set1.contains(&c) && last_char == Some(c) {
                    continue;
                }
                result.push(c);
                last_char = Some(c);
            }
        } else {
            // Translate mode
            let mut last_out: Option<char> = None;
            for c in input.chars() {
                let out = if let Some(pos) = set1.iter().position(|&sc| sc == c) {
                    if !set2.is_empty() {
                        *set2.get(pos).unwrap_or(set2.last().unwrap_or(&c))
                    } else {
                        c
                    }
                } else {
                    c
                };
                if squeeze && set2.contains(&out) && last_out == Some(out) {
                    continue;
                }
                result.push(out);
                last_out = Some(out);
            }
        }

        CommandResult {
            stdout: result,
            ..Default::default()
        }
    }
}

fn expand_tr_set(s: &str) -> Vec<char> {
    let mut result = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if i + 2 < chars.len() && chars[i + 1] == '-' {
            let start = chars[i];
            let end = chars[i + 2];
            if start <= end {
                for c in start..=end {
                    result.push(c);
                }
            } else {
                result.push(start);
                result.push('-');
                result.push(end);
            }
            i += 3;
        } else if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                'n' => result.push('\n'),
                't' => result.push('\t'),
                'r' => result.push('\r'),
                '\\' => result.push('\\'),
                other => result.push(other),
            }
            i += 2;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

// ── rev ──────────────────────────────────────────────────────────────

pub struct RevCommand;

impl super::VirtualCommand for RevCommand {
    fn name(&self) -> &str {
        "rev"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut files: Vec<&str> = Vec::new();
        let mut opts_done = false;

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            files.push(arg);
        }

        let (content, stderr, err_code) = read_all_input(&files, ctx);
        if err_code != 0 {
            return CommandResult {
                stderr,
                exit_code: err_code,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        for line in content.lines() {
            let reversed: String = line.chars().rev().collect();
            stdout.push_str(&reversed);
            stdout.push('\n');
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

// ── fold ─────────────────────────────────────────────────────────────

pub struct FoldCommand;

impl super::VirtualCommand for FoldCommand {
    fn name(&self) -> &str {
        "fold"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut width: usize = 80;
        let mut break_spaces = false;
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
            if !opts_done && arg == "-w" {
                i += 1;
                if i < args.len() {
                    width = args[i].parse().unwrap_or(80);
                }
            } else if !opts_done && arg.starts_with("-w") {
                width = arg[2..].parse().unwrap_or(80);
            } else if !opts_done && arg == "-s" {
                break_spaces = true;
            } else {
                files.push(arg);
            }
            i += 1;
        }

        let (content, stderr, err_code) = read_all_input(&files, ctx);
        if err_code != 0 {
            return CommandResult {
                stderr,
                exit_code: err_code,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        for line in content.lines() {
            if line.len() <= width {
                stdout.push_str(line);
                stdout.push('\n');
                continue;
            }

            let chars: Vec<char> = line.chars().collect();
            let mut pos = 0;
            while pos < chars.len() {
                let end = (pos + width).min(chars.len());
                if end >= chars.len() {
                    let s: String = chars[pos..].iter().collect();
                    stdout.push_str(&s);
                    stdout.push('\n');
                    break;
                }

                let break_at = if break_spaces {
                    // Find last space in window
                    let window: String = chars[pos..end].iter().collect();
                    match window.rfind(' ') {
                        Some(space_pos) if space_pos > 0 => pos + space_pos + 1,
                        _ => end,
                    }
                } else {
                    end
                };

                let s: String = chars[pos..break_at].iter().collect();
                stdout.push_str(&s);
                stdout.push('\n');
                pos = break_at;
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

// ── nl ───────────────────────────────────────────────────────────────

pub struct NlCommand;

impl super::VirtualCommand for NlCommand {
    fn name(&self) -> &str {
        "nl"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut files: Vec<&str> = Vec::new();
        let mut opts_done = false;

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                // ignore flags
            } else {
                files.push(arg);
            }
        }

        let (content, stderr, err_code) = read_all_input(&files, ctx);
        if err_code != 0 {
            return CommandResult {
                stderr,
                exit_code: err_code,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut num = 1;
        for line in content.lines() {
            if line.is_empty() {
                stdout.push_str(&format!("       {line}\n"));
            } else {
                stdout.push_str(&format!("{:>6}\t{}\n", num, line));
                num += 1;
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

// ── printf ───────────────────────────────────────────────────────────

pub struct PrintfCommand;

impl super::VirtualCommand for PrintfCommand {
    fn name(&self) -> &str {
        "printf"
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        if args.is_empty() {
            return CommandResult {
                stderr: "printf: usage: printf format [arguments]\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let format_str = &args[0];
        let arguments = &args[1..];
        let mut stdout = String::new();
        let mut arg_idx = 0;
        let arg_count = arguments.len();

        // If there are arguments, we need to cycle through the format at least once
        // If there are no arguments, we process the format once
        let need_cycle = arg_count > 0;
        let mut first_pass = true;

        loop {
            let start_arg_idx = arg_idx;
            let result = format_printf(format_str, arguments, &mut arg_idx);
            stdout.push_str(&result);

            if !need_cycle || arg_idx >= arg_count {
                break;
            }
            if !first_pass && arg_idx == start_arg_idx {
                break; // no progress, avoid infinite loop
            }
            first_pass = false;
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

fn format_printf(fmt: &str, args: &[String], arg_idx: &mut usize) -> String {
    let mut result = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\\' {
            i += 1;
            if i < chars.len() {
                match chars[i] {
                    'n' => result.push('\n'),
                    't' => result.push('\t'),
                    'r' => result.push('\r'),
                    '\\' => result.push('\\'),
                    'a' => result.push('\x07'),
                    'b' => result.push('\x08'),
                    'f' => result.push('\x0C'),
                    'v' => result.push('\x0B'),
                    '0' => {
                        // Octal escape
                        let mut val = 0u32;
                        let mut count = 0;
                        while i + 1 < chars.len()
                            && count < 3
                            && chars[i + 1].is_ascii_digit()
                            && chars[i + 1] != '8'
                            && chars[i + 1] != '9'
                        {
                            i += 1;
                            val = val * 8 + chars[i].to_digit(8).unwrap_or(0);
                            count += 1;
                        }
                        if count == 0 {
                            result.push('\0');
                        } else if let Some(c) = char::from_u32(val) {
                            result.push(c);
                        }
                    }
                    other => {
                        result.push('\\');
                        result.push(other);
                    }
                }
            }
        } else if chars[i] == '%' {
            i += 1;
            if i >= chars.len() {
                result.push('%');
                continue;
            }
            match chars[i] {
                '%' => result.push('%'),
                's' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.as_str()
                    } else {
                        ""
                    };
                    result.push_str(arg);
                }
                'd' | 'i' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<i64>().unwrap_or(0)
                    } else {
                        0
                    };
                    result.push_str(&arg.to_string());
                }
                'f' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<f64>().unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    result.push_str(&format!("{:.6}", arg));
                }
                'x' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<i64>().unwrap_or(0)
                    } else {
                        0
                    };
                    result.push_str(&format!("{:x}", arg));
                }
                'X' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<i64>().unwrap_or(0)
                    } else {
                        0
                    };
                    result.push_str(&format!("{:X}", arg));
                }
                'o' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<i64>().unwrap_or(0)
                    } else {
                        0
                    };
                    result.push_str(&format!("{:o}", arg));
                }
                'c' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.chars().next().unwrap_or('\0')
                    } else {
                        '\0'
                    };
                    if arg != '\0' {
                        result.push(arg);
                    }
                }
                _ => {
                    result.push('%');
                    result.push(chars[i]);
                }
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }
    result
}

// ── paste ────────────────────────────────────────────────────────────

pub struct PasteCommand;

impl super::VirtualCommand for PasteCommand {
    fn name(&self) -> &str {
        "paste"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut delimiter = "\t".to_string();
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
            if !opts_done && arg == "-d" {
                i += 1;
                if i < args.len() {
                    delimiter = args[i].clone();
                }
            } else if !opts_done && arg.starts_with("-d") {
                delimiter = arg[2..].to_string();
            } else {
                files.push(arg);
            }
            i += 1;
        }

        if files.is_empty() {
            files.push("-");
        }

        let inputs = match read_input(&files, ctx) {
            Ok(i) => i,
            Err(r) => return r,
        };

        let all_lines: Vec<Vec<&str>> = inputs
            .iter()
            .map(|(_, content)| content.lines().collect())
            .collect();

        let max_lines = all_lines.iter().map(|l| l.len()).max().unwrap_or(0);
        let delim_chars: Vec<char> = if delimiter.is_empty() {
            vec!['\t']
        } else {
            delimiter.chars().collect()
        };

        let mut stdout = String::new();
        for line_idx in 0..max_lines {
            for (file_idx, file_lines) in all_lines.iter().enumerate() {
                if file_idx > 0 {
                    let d = delim_chars[(file_idx - 1) % delim_chars.len()];
                    stdout.push(d);
                }
                if let Some(line) = file_lines.get(line_idx) {
                    stdout.push_str(line);
                }
            }
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
    use crate::vfs::{InMemoryFs, VirtualFs};
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    fn setup() -> (Arc<InMemoryFs>, HashMap<String, String>, ExecutionLimits) {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(Path::new("/lines.txt"), b"banana\napple\ncherry\napple\n")
            .unwrap();
        fs.write_file(Path::new("/nums.txt"), b"3\n1\n2\n10\n")
            .unwrap();
        fs.write_file(Path::new("/data.txt"), b"a:b:c\nd:e:f\n")
            .unwrap();
        fs.write_file(Path::new("/empty.txt"), b"").unwrap();
        (fs, HashMap::new(), ExecutionLimits::default())
    }

    fn ctx_with_stdin<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        stdin: &'a str,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin,
            limits,
            exec: None,
        }
    }

    fn ctx<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
    ) -> CommandContext<'a> {
        ctx_with_stdin(fs, env, limits, "")
    }

    // ── grep tests ───────────────────────────────────────────────────

    #[test]
    fn grep_basic_match() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&["apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "apple\napple\n");
    }

    #[test]
    fn grep_no_match() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&["grape".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn grep_case_insensitive() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&["-i".into(), "APPLE".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "apple\napple\n");
    }

    #[test]
    fn grep_invert() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&["-v".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "banana\ncherry\n");
    }

    #[test]
    fn grep_line_numbers() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&["-n".into(), "apple".into(), "lines.txt".into()], &c);
        assert!(r.stdout.contains("2:apple"));
        assert!(r.stdout.contains("4:apple"));
    }

    #[test]
    fn grep_count() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&["-c".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "2\n");
    }

    #[test]
    fn grep_files_with_matches() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&["-l".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "lines.txt\n");
    }

    #[test]
    fn grep_stdin() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "hello\nworld\nhello\n");
        let r = GrepCommand.execute(&["hello".into()], &c);
        assert_eq!(r.stdout, "hello\nhello\n");
    }

    #[test]
    fn grep_missing_pattern() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = GrepCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 2);
    }

    #[test]
    fn grep_fixed_string() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "a.b\na*b\n");
        let r = GrepCommand.execute(&["-F".into(), "a.b".into()], &c);
        assert_eq!(r.stdout, "a.b\n");
    }

    // ── sort tests ───────────────────────────────────────────────────

    #[test]
    fn sort_basic() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = SortCommand.execute(&["lines.txt".into()], &c);
        assert_eq!(r.stdout, "apple\napple\nbanana\ncherry\n");
    }

    #[test]
    fn sort_reverse() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = SortCommand.execute(&["-r".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "cherry\nbanana\napple\napple\n");
    }

    #[test]
    fn sort_numeric() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = SortCommand.execute(&["-n".into(), "nums.txt".into()], &c);
        assert_eq!(r.stdout, "1\n2\n3\n10\n");
    }

    #[test]
    fn sort_unique() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = SortCommand.execute(&["-u".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "apple\nbanana\ncherry\n");
    }

    #[test]
    fn sort_stdin() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "z\na\nm\n");
        let r = SortCommand.execute(&[], &c);
        assert_eq!(r.stdout, "a\nm\nz\n");
    }

    // ── uniq tests ───────────────────────────────────────────────────

    #[test]
    fn uniq_basic() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "aaa\naaa\nbbb\nccc\nccc\n");
        let r = UniqCommand.execute(&[], &c);
        assert_eq!(r.stdout, "aaa\nbbb\nccc\n");
    }

    #[test]
    fn uniq_count() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "a\na\nb\n");
        let r = UniqCommand.execute(&["-c".into()], &c);
        assert!(r.stdout.contains("2 a"));
        assert!(r.stdout.contains("1 b"));
    }

    #[test]
    fn uniq_duplicates_only() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "a\na\nb\nc\nc\n");
        let r = UniqCommand.execute(&["-d".into()], &c);
        assert_eq!(r.stdout, "a\nc\n");
    }

    #[test]
    fn uniq_unique_only() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "a\na\nb\nc\nc\n");
        let r = UniqCommand.execute(&["-u".into()], &c);
        assert_eq!(r.stdout, "b\n");
    }

    // ── cut tests ────────────────────────────────────────────────────

    #[test]
    fn cut_fields() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CutCommand.execute(
            &[
                "-d".into(),
                ":".into(),
                "-f".into(),
                "2".into(),
                "data.txt".into(),
            ],
            &c,
        );
        assert_eq!(r.stdout, "b\ne\n");
    }

    #[test]
    fn cut_characters() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "hello\nworld\n");
        let r = CutCommand.execute(&["-c".into(), "1-3".into()], &c);
        assert_eq!(r.stdout, "hel\nwor\n");
    }

    #[test]
    fn cut_missing_spec() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CutCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── head tests ───────────────────────────────────────────────────

    #[test]
    fn head_default() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(
            &*fs,
            &env,
            &limits,
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n",
        );
        let r = HeadCommand.execute(&[], &c);
        let lines: Vec<&str> = r.stdout.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "1");
        assert_eq!(lines[9], "10");
    }

    #[test]
    fn head_n3() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = HeadCommand.execute(&["-n".into(), "2".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "banana\napple\n");
    }

    #[test]
    fn head_file() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = HeadCommand.execute(&["-n".into(), "1".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "banana\n");
    }

    // ── tail tests ───────────────────────────────────────────────────

    #[test]
    fn tail_default() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(
            &*fs,
            &env,
            &limits,
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n",
        );
        let r = TailCommand.execute(&[], &c);
        let lines: Vec<&str> = r.stdout.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "3");
        assert_eq!(lines[9], "12");
    }

    #[test]
    fn tail_n2() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = TailCommand.execute(&["-n".into(), "2".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "cherry\napple\n");
    }

    // ── wc tests ─────────────────────────────────────────────────────

    #[test]
    fn wc_all() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "hello world\nfoo\n");
        let r = WcCommand.execute(&[], &c);
        assert!(r.stdout.contains("2")); // lines
        assert!(r.stdout.contains("3")); // words
    }

    #[test]
    fn wc_lines_only() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "a\nb\nc\n");
        let r = WcCommand.execute(&["-l".into()], &c);
        assert!(r.stdout.contains("3"));
    }

    #[test]
    fn wc_file() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = WcCommand.execute(&["-l".into(), "lines.txt".into()], &c);
        assert!(r.stdout.contains("4"));
    }

    // ── tr tests ─────────────────────────────────────────────────────

    #[test]
    fn tr_translate() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "hello");
        let r = TrCommand.execute(&["a-z".into(), "A-Z".into()], &c);
        assert_eq!(r.stdout, "HELLO");
    }

    #[test]
    fn tr_delete() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "hello world");
        let r = TrCommand.execute(&["-d".into(), " ".into()], &c);
        assert_eq!(r.stdout, "helloworld");
    }

    #[test]
    fn tr_squeeze() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "aabbcc");
        let r = TrCommand.execute(&["-s".into(), "a-z".into()], &c);
        assert_eq!(r.stdout, "abc");
    }

    #[test]
    fn tr_missing_operand() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = TrCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── rev tests ────────────────────────────────────────────────────

    #[test]
    fn rev_basic() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "hello\nworld\n");
        let r = RevCommand.execute(&[], &c);
        assert_eq!(r.stdout, "olleh\ndlrow\n");
    }

    // ── fold tests ───────────────────────────────────────────────────

    #[test]
    fn fold_default_width() {
        let (fs, env, limits) = setup();
        let short = "short\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, short);
        let r = FoldCommand.execute(&[], &c);
        assert_eq!(r.stdout, "short\n");
    }

    #[test]
    fn fold_custom_width() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "abcdefghij\n");
        let r = FoldCommand.execute(&["-w".into(), "5".into()], &c);
        assert_eq!(r.stdout, "abcde\nfghij\n");
    }

    // ── nl tests ─────────────────────────────────────────────────────

    #[test]
    fn nl_basic() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "first\nsecond\n");
        let r = NlCommand.execute(&[], &c);
        assert!(r.stdout.contains("1\tfirst"));
        assert!(r.stdout.contains("2\tsecond"));
    }

    #[test]
    fn nl_empty_line_not_numbered() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "a\n\nb\n");
        let r = NlCommand.execute(&[], &c);
        assert!(r.stdout.contains("1\ta"));
        assert!(r.stdout.contains("2\tb"));
    }

    // ── printf tests ─────────────────────────────────────────────────

    #[test]
    fn printf_string() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = PrintfCommand.execute(&["hello %s\n".into(), "world".into()], &c);
        assert_eq!(r.stdout, "hello world\n");
    }

    #[test]
    fn printf_int() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = PrintfCommand.execute(&["%d\n".into(), "42".into()], &c);
        assert_eq!(r.stdout, "42\n");
    }

    #[test]
    fn printf_hex() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = PrintfCommand.execute(&["%x\n".into(), "255".into()], &c);
        assert_eq!(r.stdout, "ff\n");
    }

    #[test]
    fn printf_octal() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = PrintfCommand.execute(&["%o\n".into(), "8".into()], &c);
        assert_eq!(r.stdout, "10\n");
    }

    #[test]
    fn printf_percent() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = PrintfCommand.execute(&["100%%\n".into()], &c);
        assert_eq!(r.stdout, "100%\n");
    }

    #[test]
    fn printf_no_args() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = PrintfCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn printf_multiple_args_cycle() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = PrintfCommand.execute(&["%s\n".into(), "a".into(), "b".into(), "c".into()], &c);
        assert_eq!(r.stdout, "a\nb\nc\n");
    }

    // ── paste tests ──────────────────────────────────────────────────

    #[test]
    fn paste_basic() {
        let (fs, env, limits) = setup();
        fs.write_file(Path::new("/p1.txt"), b"a\nb\n").unwrap();
        fs.write_file(Path::new("/p2.txt"), b"1\n2\n").unwrap();
        let c = ctx(&*fs, &env, &limits);
        let r = PasteCommand.execute(&["p1.txt".into(), "p2.txt".into()], &c);
        assert_eq!(r.stdout, "a\t1\nb\t2\n");
    }

    #[test]
    fn paste_custom_delimiter() {
        let (fs, env, limits) = setup();
        fs.write_file(Path::new("/p1.txt"), b"a\nb\n").unwrap();
        fs.write_file(Path::new("/p2.txt"), b"1\n2\n").unwrap();
        let c = ctx(&*fs, &env, &limits);
        let r = PasteCommand.execute(
            &["-d".into(), ",".into(), "p1.txt".into(), "p2.txt".into()],
            &c,
        );
        assert_eq!(r.stdout, "a,1\nb,2\n");
    }

    #[test]
    fn paste_stdin() {
        let (fs, env, limits) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, "x\ny\n");
        let r = PasteCommand.execute(&[], &c);
        assert_eq!(r.stdout, "x\ny\n");
    }
}
