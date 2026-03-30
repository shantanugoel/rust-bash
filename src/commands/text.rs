//! Text processing commands: grep, sort, uniq, cut, head, tail, wc, tr, rev, fold, nl, printf, paste

use super::CommandMeta;
use crate::commands::{CommandContext, CommandResult};
use crate::interpreter::pattern::glob_match;
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
        if *file == "-" || *file == "/dev/stdin" {
            result.push(("(standard input)".to_string(), ctx.stdin.to_string()));
        } else if *file == "/dev/null"
            || *file == "/dev/zero"
            || *file == "/dev/full"
            || *file == "/dev/stdout"
            || *file == "/dev/stderr"
        {
            result.push((file.to_string(), String::new()));
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
            if *file == "-" || *file == "/dev/stdin" {
                content.push_str(ctx.stdin);
            } else if *file == "/dev/null" || *file == "/dev/zero" || *file == "/dev/full" {
                // Empty content for these special devices
            } else if *file == "/dev/stdout" || *file == "/dev/stderr" {
                // Reading from stdout/stderr produces empty content
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegexMode {
    Extended,
    Basic,
    Fixed,
}

#[derive(Debug, Default)]
struct GrepOpts<'a> {
    case_insensitive: bool,
    invert: bool,
    show_line_numbers: bool,
    count_only: bool,
    files_with_matches: bool,
    files_without_match: bool,
    regex_mode: Option<RegexMode>,
    word_regexp: bool,
    line_regexp: bool,
    recursive: bool,
    only_matching: bool,
    force_filename: Option<bool>,
    quiet: bool,
    max_count: Option<usize>,
    after_context: usize,
    before_context: usize,
    context_requested: bool,
    include_globs: Vec<&'a str>,
    exclude_globs: Vec<&'a str>,
    patterns: Vec<&'a str>,
    pattern_files: Vec<&'a str>,
    files: Vec<&'a str>,
    perl_warned: bool,
}

fn parse_grep_args<'a>(args: &'a [String]) -> Result<GrepOpts<'a>, CommandResult> {
    let mut opts = GrepOpts::default();
    let mut opts_done = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if opts_done || !arg.starts_with('-') || arg == "-" {
            if opts.patterns.is_empty() && opts.pattern_files.is_empty() {
                opts.patterns.push(arg);
            } else {
                opts.files.push(arg);
            }
            i += 1;
            continue;
        }

        if arg == "--" {
            opts_done = true;
            i += 1;
            continue;
        }

        // Long flags
        if let Some(flag) = arg.strip_prefix("--") {
            if let Some(val) = flag.strip_prefix("include=") {
                opts.include_globs.push(val);
            } else if let Some(val) = flag.strip_prefix("exclude=") {
                opts.exclude_globs.push(val);
            } else if let Some(val) = flag.strip_prefix("after-context=") {
                opts.after_context = parse_num_arg(val, "-A")?;
                opts.context_requested = true;
            } else if let Some(val) = flag.strip_prefix("before-context=") {
                opts.before_context = parse_num_arg(val, "-B")?;
                opts.context_requested = true;
            } else if let Some(val) = flag.strip_prefix("context=") {
                let n = parse_num_arg(val, "-C")?;
                opts.after_context = n;
                opts.before_context = n;
                opts.context_requested = true;
            } else if let Some(val) = flag.strip_prefix("max-count=") {
                opts.max_count = Some(parse_num_arg(val, "-m")?);
            } else if let Some(val) = flag.strip_prefix("file=") {
                opts.pattern_files.push(val);
            } else {
                match flag {
                    "extended-regexp" => opts.regex_mode = Some(RegexMode::Extended),
                    "basic-regexp" => {
                        opts.regex_mode = Some(RegexMode::Basic);
                    }
                    "perl-regexp" => {
                        opts.regex_mode = Some(RegexMode::Extended);
                        opts.perl_warned = true;
                    }
                    "fixed-strings" => opts.regex_mode = Some(RegexMode::Fixed),
                    "ignore-case" => opts.case_insensitive = true,
                    "invert-match" => opts.invert = true,
                    "line-number" => opts.show_line_numbers = true,
                    "count" => opts.count_only = true,
                    "files-with-matches" => opts.files_with_matches = true,
                    "files-without-match" => opts.files_without_match = true,
                    "word-regexp" => opts.word_regexp = true,
                    "line-regexp" => opts.line_regexp = true,
                    "recursive" => opts.recursive = true,
                    "only-matching" => opts.only_matching = true,
                    "with-filename" => opts.force_filename = Some(true),
                    "no-filename" => opts.force_filename = Some(false),
                    "quiet" | "silent" => opts.quiet = true,
                    _ => {
                        return Err(CommandResult {
                            stderr: format!("grep: unrecognized option '--{}'\n", flag),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                }
            }
            i += 1;
            continue;
        }

        // Short flags — may be combined (e.g., -inl) unless a flag takes a value
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut j = 0;
        while j < chars.len() {
            match chars[j] {
                'i' => opts.case_insensitive = true,
                'v' => opts.invert = true,
                'n' => opts.show_line_numbers = true,
                'c' => opts.count_only = true,
                'l' => opts.files_with_matches = true,
                'L' => opts.files_without_match = true,
                'F' => opts.regex_mode = Some(RegexMode::Fixed),
                'E' => opts.regex_mode = Some(RegexMode::Extended),
                'G' => opts.regex_mode = Some(RegexMode::Basic),
                'P' => {
                    opts.regex_mode = Some(RegexMode::Extended);
                    opts.perl_warned = true;
                }
                'w' => opts.word_regexp = true,
                'x' => opts.line_regexp = true,
                'r' | 'R' => opts.recursive = true,
                'o' => opts.only_matching = true,
                'H' => opts.force_filename = Some(true),
                'h' => opts.force_filename = Some(false),
                'q' => opts.quiet = true,
                // Flags that consume the rest of the combined flag or the next arg as a value
                'e' => {
                    let val = get_flag_value(&chars, j, i, args, "-e")?;
                    opts.patterns.push(val);
                    if j + 1 < chars.len() {
                        // consumed rest of current arg
                        j = chars.len();
                        continue;
                    } else {
                        i += 1; // consumed next arg
                        j = chars.len();
                        continue;
                    }
                }
                'f' => {
                    let val = get_flag_value(&chars, j, i, args, "-f")?;
                    opts.pattern_files.push(val);
                    if j + 1 < chars.len() {
                        j = chars.len();
                        continue;
                    } else {
                        i += 1;
                        j = chars.len();
                        continue;
                    }
                }
                'A' => {
                    let val = get_flag_value(&chars, j, i, args, "-A")?;
                    opts.after_context = parse_num_arg(val, "-A")?;
                    opts.context_requested = true;
                    if j + 1 < chars.len() {
                        j = chars.len();
                        continue;
                    } else {
                        i += 1;
                        j = chars.len();
                        continue;
                    }
                }
                'B' => {
                    let val = get_flag_value(&chars, j, i, args, "-B")?;
                    opts.before_context = parse_num_arg(val, "-B")?;
                    opts.context_requested = true;
                    if j + 1 < chars.len() {
                        j = chars.len();
                        continue;
                    } else {
                        i += 1;
                        j = chars.len();
                        continue;
                    }
                }
                'C' => {
                    let val = get_flag_value(&chars, j, i, args, "-C")?;
                    let n = parse_num_arg(val, "-C")?;
                    opts.after_context = n;
                    opts.before_context = n;
                    opts.context_requested = true;
                    if j + 1 < chars.len() {
                        j = chars.len();
                        continue;
                    } else {
                        i += 1;
                        j = chars.len();
                        continue;
                    }
                }
                'm' => {
                    let val = get_flag_value(&chars, j, i, args, "-m")?;
                    opts.max_count = Some(parse_num_arg(val, "-m")?);
                    if j + 1 < chars.len() {
                        j = chars.len();
                        continue;
                    } else {
                        i += 1;
                        j = chars.len();
                        continue;
                    }
                }
                _ => {
                    return Err(CommandResult {
                        stderr: format!("grep: invalid option -- '{}'\n", chars[j]),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
            }
            j += 1;
        }
        i += 1;
    }

    Ok(opts)
}

/// Extract the value for a flag that takes an argument. If there are remaining
/// chars after `j` in the combined flag, those chars are the value. Otherwise
/// the next arg is consumed.
fn get_flag_value<'a>(
    chars: &[char],
    j: usize,
    i: usize,
    args: &'a [String],
    flag_name: &str,
) -> Result<&'a str, CommandResult> {
    if j + 1 < chars.len() {
        // Rest of this arg is the value — but we need a reference into args
        // The value starts at byte offset corresponding to chars[j+1..]
        let prefix_len: usize = 1 + chars[..=j].iter().map(|c| c.len_utf8()).sum::<usize>();
        Ok(&args[i][prefix_len..])
    } else if i + 1 < args.len() {
        Ok(&args[i + 1])
    } else {
        Err(CommandResult {
            stderr: format!("grep: option requires an argument -- '{}'\n", flag_name),
            exit_code: 2,
            ..Default::default()
        })
    }
}

fn parse_num_arg(val: &str, flag_name: &str) -> Result<usize, CommandResult> {
    val.parse::<usize>().map_err(|_| CommandResult {
        stderr: format!("grep: invalid argument '{}' for '{}'\n", val, flag_name),
        exit_code: 2,
        ..Default::default()
    })
}

/// Collect files for grep, expanding directories recursively if needed.
fn collect_grep_files<'a>(
    files: &[&'a str],
    recursive: bool,
    include_globs: &[&str],
    exclude_globs: &[&str],
    ctx: &'a CommandContext,
) -> Result<Vec<(String, String)>, CommandResult> {
    if files.is_empty() {
        if recursive {
            // Recursive with no files: search current directory
            let mut result = Vec::new();
            collect_dir_recursive(
                &PathBuf::from(ctx.cwd),
                include_globs,
                exclude_globs,
                ctx,
                &mut result,
            )?;
            result.sort_by(|a, b| a.0.cmp(&b.0));
            return Ok(result);
        }
        return Ok(vec![(
            "(standard input)".to_string(),
            ctx.stdin.to_string(),
        )]);
    }

    let mut result = Vec::new();
    for file in files {
        if *file == "-" {
            result.push(("(standard input)".to_string(), ctx.stdin.to_string()));
            continue;
        }
        let path = resolve_path(file, ctx.cwd);
        if recursive {
            let stat = ctx.fs.stat(&path).map_err(|e| CommandResult {
                stderr: format!("grep: {}: {}\n", file, e),
                exit_code: 2,
                ..Default::default()
            })?;
            if stat.node_type == crate::vfs::NodeType::Directory {
                collect_dir_recursive(&path, include_globs, exclude_globs, ctx, &mut result)?;
                continue;
            }
        }
        // Only apply include/exclude filters during recursive traversal
        if recursive && !matches_glob_filters(file, include_globs, exclude_globs) {
            continue;
        }
        match ctx.fs.read_file(&path) {
            Ok(bytes) => {
                result.push((
                    file.to_string(),
                    String::from_utf8_lossy(&bytes).to_string(),
                ));
            }
            Err(e) => {
                return Err(CommandResult {
                    stderr: format!("grep: {}: {}\n", file, e),
                    exit_code: 2,
                    ..Default::default()
                });
            }
        }
    }
    Ok(result)
}

fn collect_dir_recursive(
    dir: &std::path::Path,
    include_globs: &[&str],
    exclude_globs: &[&str],
    ctx: &CommandContext,
    result: &mut Vec<(String, String)>,
) -> Result<(), CommandResult> {
    let entries = ctx.fs.readdir(dir).map_err(|e| CommandResult {
        stderr: format!("grep: {}: {}\n", dir.display(), e),
        exit_code: 2,
        ..Default::default()
    })?;

    let mut sorted_entries = entries;
    sorted_entries.sort_by(|a, b| a.name.cmp(&b.name));

    for entry in &sorted_entries {
        let child = dir.join(&entry.name);
        match entry.node_type {
            crate::vfs::NodeType::Directory => {
                collect_dir_recursive(&child, include_globs, exclude_globs, ctx, result)?;
            }
            crate::vfs::NodeType::File => {
                let name_str = child.to_string_lossy().to_string();
                if !matches_glob_filters(&entry.name, include_globs, exclude_globs) {
                    continue;
                }
                match ctx.fs.read_file(&child) {
                    Ok(bytes) => {
                        result.push((name_str, String::from_utf8_lossy(&bytes).to_string()));
                    }
                    Err(e) => {
                        return Err(CommandResult {
                            stderr: format!("grep: {}: {}\n", name_str, e),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                }
            }
            _ => {} // skip symlinks etc.
        }
    }
    Ok(())
}

fn matches_glob_filters(filename: &str, include_globs: &[&str], exclude_globs: &[&str]) -> bool {
    // Extract just the filename component for matching
    let basename = std::path::Path::new(filename)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());

    if !include_globs.is_empty() && !include_globs.iter().any(|g| glob_match(g, &basename)) {
        return false;
    }
    if exclude_globs.iter().any(|g| glob_match(g, &basename)) {
        return false;
    }
    true
}

static GREP_FLAGS: &[super::FlagInfo] = &[
    super::FlagInfo {
        flag: "-i",
        description: "ignore case distinctions",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-v",
        description: "invert match",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-n",
        description: "line numbers",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-c",
        description: "count matching lines",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-l",
        description: "files with matches",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-L",
        description: "files without matches",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-F",
        description: "fixed strings",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-E",
        description: "extended regex",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-P",
        description: "perl regex",
        status: super::FlagStatus::Stubbed,
    },
    super::FlagInfo {
        flag: "-w",
        description: "word regexp",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-x",
        description: "line regexp",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-r",
        description: "recursive search",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-o",
        description: "only matching part",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-H",
        description: "with filename",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-h",
        description: "no filename",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-q",
        description: "quiet mode",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-A",
        description: "after context lines",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-B",
        description: "before context lines",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-C",
        description: "context lines",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-m",
        description: "max match count",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "--include",
        description: "include file glob",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "--exclude",
        description: "exclude file glob",
        status: super::FlagStatus::Supported,
    },
];

static GREP_META: CommandMeta = CommandMeta {
    name: "grep",
    synopsis: "grep [OPTIONS] PATTERN [FILE ...]",
    description: "Print lines that match patterns.",
    options: &[
        ("-i, --ignore-case", "ignore case distinctions"),
        ("-v, --invert-match", "select non-matching lines"),
        ("-n, --line-number", "prefix each line with line number"),
        ("-c, --count", "print only a count of matching lines"),
        (
            "-l, --files-with-matches",
            "print only names of files with matches",
        ),
        (
            "-L, --files-without-match",
            "print only names of files without matches",
        ),
        ("-F, --fixed-strings", "interpret pattern as fixed string"),
        (
            "-E, --extended-regexp",
            "interpret pattern as extended regex",
        ),
        ("-P, --perl-regexp", "interpret pattern as Perl regex"),
        ("-w, --word-regexp", "match only whole words"),
        ("-x, --line-regexp", "match only whole lines"),
        ("-r, -R, --recursive", "search directories recursively"),
        (
            "-o, --only-matching",
            "show only the matching part of lines",
        ),
        ("-H, --with-filename", "print the file name for each match"),
        ("-h, --no-filename", "suppress the file name prefix"),
        ("-q, --quiet", "suppress all normal output"),
        ("-e PATTERN", "use PATTERN for matching"),
        ("-f FILE", "obtain patterns from FILE"),
        ("-A NUM", "print NUM lines of trailing context"),
        ("-B NUM", "print NUM lines of leading context"),
        ("-C NUM", "print NUM lines of output context"),
        ("-m NUM, --max-count=NUM", "stop after NUM matches"),
        ("--include=GLOB", "search only files matching GLOB"),
        ("--exclude=GLOB", "skip files matching GLOB"),
    ],
    supports_help_flag: true,
    flags: GREP_FLAGS,
};

impl super::VirtualCommand for GrepCommand {
    fn name(&self) -> &str {
        "grep"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&GREP_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let opts = match parse_grep_args(args) {
            Ok(o) => o,
            Err(r) => return r,
        };

        let mut stderr = String::new();

        if opts.perl_warned {
            stderr.push_str("grep: warning: -P is not fully supported, using extended regex\n");
        }

        // Load patterns from files
        let mut file_patterns: Vec<String> = Vec::new();
        for pf in &opts.pattern_files {
            let path = resolve_path(pf, ctx.cwd);
            match ctx.fs.read_file(&path) {
                Ok(bytes) => {
                    let content = String::from_utf8_lossy(&bytes);
                    for line in content.lines() {
                        file_patterns.push(line.to_string());
                    }
                }
                Err(e) => {
                    return CommandResult {
                        stderr: format!("grep: {}: {}\n", pf, e),
                        exit_code: 2,
                        ..Default::default()
                    };
                }
            }
        }

        // If no patterns from -e or -f, the first positional arg is the pattern
        if opts.patterns.is_empty() && file_patterns.is_empty() {
            return CommandResult {
                stderr: "grep: missing pattern\n".into(),
                exit_code: 2,
                ..Default::default()
            };
        }

        let regex_mode = opts.regex_mode.unwrap_or(RegexMode::Basic);

        // Build combined regex from all patterns
        let all_patterns: Vec<String> = opts
            .patterns
            .iter()
            .map(|p| p.to_string())
            .chain(file_patterns)
            .collect();

        let regex_parts: Vec<String> = all_patterns
            .iter()
            .map(|p| build_pattern(p, regex_mode, opts.word_regexp, opts.line_regexp))
            .collect();

        let combined_pattern = if regex_parts.len() == 1 {
            regex_parts.into_iter().next().unwrap()
        } else {
            regex_parts
                .iter()
                .map(|p| format!("(?:{})", p))
                .collect::<Vec<_>>()
                .join("|")
        };

        let final_pattern = if opts.case_insensitive {
            format!("(?i){}", combined_pattern)
        } else {
            combined_pattern
        };

        let re = match Regex::new(&final_pattern) {
            Ok(r) => r,
            Err(e) => {
                return CommandResult {
                    stderr: format!("grep: invalid regex: {}\n", e),
                    exit_code: 2,
                    ..Default::default()
                };
            }
        };

        let inputs = match collect_grep_files(
            &opts.files,
            opts.recursive,
            &opts.include_globs,
            &opts.exclude_globs,
            ctx,
        ) {
            Ok(i) => i,
            Err(r) => return r,
        };

        let show_filename = match opts.force_filename {
            Some(v) => v,
            None => inputs.len() > 1 || opts.recursive,
        };

        let mut stdout = String::new();
        let mut any_match = false;
        let mut any_file_without_match = false;
        let has_context = opts.context_requested;

        for (filename, content) in &inputs {
            let lines: Vec<&str> = content.lines().collect();
            let mut file_match_count: usize = 0;
            let mut file_matched = false;

            if has_context
                && !opts.count_only
                && !opts.files_with_matches
                && !opts.files_without_match
                && !opts.quiet
                && !opts.only_matching
            {
                // Context mode: track which lines to print
                let cr = grep_with_context(&lines, &re, &opts, filename, show_filename);
                file_match_count = cr.match_count;
                file_matched = cr.had_match;
                if cr.had_match {
                    any_match = true;
                }
                stdout.push_str(&cr.output);
            } else {
                // Non-context mode
                for (line_idx, line) in lines.iter().enumerate() {
                    if opts.max_count.is_some_and(|mc| file_match_count >= mc) {
                        break;
                    }

                    let matched = re.is_match(line);
                    let matched = if opts.invert { !matched } else { matched };

                    if matched {
                        file_match_count += 1;
                        file_matched = true;
                        any_match = true;

                        if opts.quiet {
                            return CommandResult {
                                stdout: String::new(),
                                stderr,
                                exit_code: 0,
                            };
                        }

                        if !opts.count_only && !opts.files_with_matches && !opts.files_without_match
                        {
                            if opts.only_matching && opts.invert {
                                // -o with -v: inverted lines have no match to extract
                            } else if opts.only_matching {
                                // Print each match on the line
                                for mat in re.find_iter(line) {
                                    if show_filename {
                                        stdout.push_str(filename);
                                        stdout.push(':');
                                    }
                                    if opts.show_line_numbers {
                                        stdout.push_str(&(line_idx + 1).to_string());
                                        stdout.push(':');
                                    }
                                    stdout.push_str(mat.as_str());
                                    stdout.push('\n');
                                }
                            } else {
                                format_match_line(
                                    &mut stdout,
                                    filename,
                                    line_idx + 1,
                                    line,
                                    ':',
                                    show_filename,
                                    opts.show_line_numbers,
                                );
                            }
                        }
                    }
                }
            }

            if opts.count_only && !opts.quiet {
                if show_filename {
                    stdout.push_str(&format!("{}:{}\n", filename, file_match_count));
                } else {
                    stdout.push_str(&format!("{}\n", file_match_count));
                }
            }

            if opts.files_with_matches && file_matched && !opts.quiet {
                stdout.push_str(filename);
                stdout.push('\n');
            }

            if opts.files_without_match && !file_matched {
                stdout.push_str(filename);
                stdout.push('\n');
                any_file_without_match = true;
            }
        }

        let exit_match = if opts.files_without_match {
            any_file_without_match
        } else {
            any_match
        };

        CommandResult {
            stdout,
            stderr,
            exit_code: if exit_match { 0 } else { 1 },
        }
    }
}

fn build_pattern(
    pattern: &str,
    regex_mode: RegexMode,
    word_regexp: bool,
    line_regexp: bool,
) -> String {
    let mut p = match regex_mode {
        RegexMode::Fixed => regex::escape(pattern),
        RegexMode::Basic => crate::commands::regex_util::bre_to_ere(pattern),
        RegexMode::Extended => pattern.to_string(),
    };

    if word_regexp {
        p = format!(r"\b{}\b", p);
    }
    if line_regexp {
        p = format!("^{}$", p);
    }
    p
}

fn format_match_line(
    out: &mut String,
    filename: &str,
    line_num: usize,
    line: &str,
    sep: char,
    show_filename: bool,
    show_line_numbers: bool,
) {
    if show_filename {
        out.push_str(filename);
        out.push(sep);
    }
    if show_line_numbers {
        out.push_str(&line_num.to_string());
        out.push(sep);
    }
    out.push_str(line);
    out.push('\n');
}

struct ContextResult {
    output: String,
    match_count: usize,
    had_match: bool,
}

fn grep_with_context(
    lines: &[&str],
    re: &Regex,
    opts: &GrepOpts,
    filename: &str,
    show_filename: bool,
) -> ContextResult {
    let n = lines.len();
    let mut match_count: usize = 0;
    let mut had_match = false;

    // Determine which lines are matches
    let mut is_match = vec![false; n];
    for (idx, line) in lines.iter().enumerate() {
        if opts.max_count.is_some_and(|mc| match_count >= mc) {
            break;
        }
        let matched = re.is_match(line);
        let matched = if opts.invert { !matched } else { matched };
        if matched {
            is_match[idx] = true;
            match_count += 1;
            had_match = true;
        }
    }

    // Calculate which lines should be printed (match lines + context)
    let mut print_line = vec![false; n];
    for (idx, matched) in is_match.iter().enumerate() {
        if *matched {
            let start = idx.saturating_sub(opts.before_context);
            let end = (idx + opts.after_context + 1).min(n);
            for flag in &mut print_line[start..end] {
                *flag = true;
            }
        }
    }

    // Print with group separators
    let mut output = String::new();
    let mut last_printed: Option<usize> = None;
    for (idx, (&should_print, &matched)) in print_line.iter().zip(is_match.iter()).enumerate() {
        if !should_print {
            continue;
        }
        if last_printed.is_some_and(|lp| idx > lp + 1) {
            output.push_str("--\n");
        }
        let sep = if matched { ':' } else { '-' };
        format_match_line(
            &mut output,
            filename,
            idx + 1,
            lines[idx],
            sep,
            show_filename,
            opts.show_line_numbers,
        );
        last_printed = Some(idx);
    }

    ContextResult {
        output,
        match_count,
        had_match,
    }
}

// ── sort ─────────────────────────────────────────────────────────────

pub struct SortCommand;

static SORT_FLAGS: &[super::FlagInfo] = &[
    super::FlagInfo {
        flag: "-r",
        description: "reverse sort order",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-n",
        description: "numeric sort",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-u",
        description: "unique lines only",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-k",
        description: "sort key field",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-t",
        description: "field separator",
        status: super::FlagStatus::Supported,
    },
    super::FlagInfo {
        flag: "-f",
        description: "fold lower to upper case",
        status: super::FlagStatus::Ignored,
    },
    super::FlagInfo {
        flag: "-s",
        description: "stable sort",
        status: super::FlagStatus::Ignored,
    },
];

static SORT_META: CommandMeta = CommandMeta {
    name: "sort",
    synopsis: "sort [-rnuk KEY] [-t SEP] [FILE ...]",
    description: "Sort lines of text files.",
    options: &[
        ("-r", "reverse the result of comparisons"),
        ("-n", "compare according to string numerical value"),
        ("-u", "output only unique lines"),
        ("-k KEY", "sort via a key field specification"),
        ("-t SEP", "use SEP as the field separator"),
    ],
    supports_help_flag: true,
    flags: SORT_FLAGS,
};

impl super::VirtualCommand for SortCommand {
    fn name(&self) -> &str {
        "sort"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&SORT_META)
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
            if !opts_done && arg.starts_with("--") {
                return super::unknown_option("sort", arg);
            } else if !opts_done && arg.starts_with('-') && arg.len() > 1 {
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
                        'f' | 's' => {} // accepted but silently ignored
                        _ => {
                            return super::unknown_option("sort", &format!("-{}", c));
                        }
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
                let an = parse_leading_number(a_key);
                let bn = parse_leading_number(b_key);
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

/// Extract the leading numeric value from a string, matching `sort -n` behavior.
fn parse_leading_number(s: &str) -> f64 {
    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        return 0.0;
    }
    let mut end = 0;
    let bytes = trimmed.as_bytes();
    if end < bytes.len() && (bytes[end] == b'-' || bytes[end] == b'+') {
        end += 1;
    }
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
    }
    trimmed[..end].parse::<f64>().unwrap_or(0.0)
}

// ── uniq ─────────────────────────────────────────────────────────────

pub struct UniqCommand;

static UNIQ_META: CommandMeta = CommandMeta {
    name: "uniq",
    synopsis: "uniq [-cdu] [FILE]",
    description: "Report or omit repeated lines.",
    options: &[
        ("-c", "prefix lines by the number of occurrences"),
        ("-d", "only print duplicate lines"),
        ("-u", "only print unique lines"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for UniqCommand {
    fn name(&self) -> &str {
        "uniq"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&UNIQ_META)
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

static CUT_META: CommandMeta = CommandMeta {
    name: "cut",
    synopsis: "cut -f FIELDS [-d DELIM] [FILE ...]",
    description: "Remove sections from each line of files.",
    options: &[
        ("-d DELIM", "use DELIM instead of TAB for field delimiter"),
        ("-f FIELDS", "select only these fields"),
        ("-c CHARS", "select only these character positions"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for CutCommand {
    fn name(&self) -> &str {
        "cut"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&CUT_META)
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

static HEAD_META: CommandMeta = CommandMeta {
    name: "head",
    synopsis: "head [-n NUM] [-c NUM] [FILE ...]",
    description: "Output the first part of files.",
    options: &[
        ("-n NUM", "print the first NUM lines (default 10)"),
        ("-c NUM", "print the first NUM bytes"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for HeadCommand {
    fn name(&self) -> &str {
        "head"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&HEAD_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut num_lines: usize = 10;
        let mut num_bytes: Option<usize> = None;
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
            } else if !opts_done && arg == "-c" {
                i += 1;
                if i < args.len() {
                    num_bytes = Some(args[i].parse().unwrap_or(0));
                }
            } else if !opts_done && arg.starts_with("-c") {
                num_bytes = Some(arg[2..].parse().unwrap_or(0));
            } else if !opts_done
                && arg.starts_with('-')
                && arg.len() > 1
                && arg[1..].chars().all(|c| c.is_ascii_digit())
            {
                num_lines = arg[1..].parse().unwrap_or(10);
            } else if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                return super::unknown_option("head", arg);
            } else {
                files.push(arg);
            }
            i += 1;
        }

        // Special handling for /dev/zero with -c (produces null bytes)
        if let Some(count) = num_bytes
            && files.contains(&"/dev/zero")
        {
            let bytes = vec![0u8; count];
            return CommandResult {
                stdout: String::from_utf8_lossy(&bytes).to_string(),
                ..Default::default()
            };
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
            if let Some(count) = num_bytes {
                // Byte count mode
                let bytes: String = content.chars().take(count).collect();
                stdout.push_str(&bytes);
            } else {
                for line in content.lines().take(num_lines) {
                    stdout.push_str(line);
                    stdout.push('\n');
                }
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

static TAIL_META: CommandMeta = CommandMeta {
    name: "tail",
    synopsis: "tail [-n NUM] [FILE ...]",
    description: "Output the last part of files.",
    options: &[("-n NUM", "print the last NUM lines (default 10)")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for TailCommand {
    fn name(&self) -> &str {
        "tail"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TAIL_META)
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
            } else if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                return super::unknown_option("tail", arg);
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

// ── od ──────────────────────────────────────────────────────────────

pub struct OdCommand;

static OD_META: CommandMeta = CommandMeta {
    name: "od",
    synopsis: "od [-An] [-t TYPE] [FILE ...]",
    description: "Dump files in octal and other formats.",
    options: &[
        ("-An", "suppress the address column"),
        ("-t TYPE", "select output format (e.g. x1, o2)"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for OdCommand {
    fn name(&self) -> &str {
        "od"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&OD_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut no_address = false;
        let mut format = "o2".to_string(); // default: octal 2-byte
        let mut files: Vec<&str> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            if arg == "-An" || arg == "-A" && args.get(i + 1).map(|s| s.as_str()) == Some("n") {
                no_address = true;
                if arg == "-A" {
                    i += 1; // skip "n"
                }
            } else if arg.starts_with("-A") {
                // -Ax, -Ad, -Ao — we only handle -An (suppress address)
                if arg == "-An" {
                    no_address = true;
                }
            } else if let Some(suffix) = arg.strip_prefix("-t") {
                if !suffix.is_empty() {
                    format = suffix.to_string();
                } else {
                    i += 1;
                    if i < args.len() {
                        format = args[i].clone();
                    }
                }
            } else if !arg.starts_with('-') {
                files.push(arg);
            }
            i += 1;
        }

        // Read input bytes
        let input_str = if files.is_empty() {
            ctx.stdin.to_string()
        } else {
            let mut s = String::new();
            for file in &files {
                if *file == "-" || *file == "/dev/stdin" {
                    s.push_str(ctx.stdin);
                } else if *file == "/dev/zero" {
                    // /dev/zero content should come via pipe
                    s.push_str(ctx.stdin);
                } else {
                    let path = resolve_path(file, ctx.cwd);
                    match ctx.fs.read_file(&path) {
                        Ok(bytes) => s.push_str(&String::from_utf8_lossy(&bytes)),
                        Err(e) => {
                            return CommandResult {
                                stderr: format!("od: {file}: {e}\n"),
                                exit_code: 1,
                                ..Default::default()
                            };
                        }
                    }
                }
            }
            s
        };

        let bytes = input_str.as_bytes();
        let mut stdout = String::new();

        // Format bytes according to type specifier
        if format == "x1" {
            // Hex, 1-byte units
            let mut offset = 0;
            while offset < bytes.len() {
                if !no_address {
                    stdout.push_str(&format!("{:07o}", offset));
                }
                let end = std::cmp::min(offset + 16, bytes.len());
                for b in &bytes[offset..end] {
                    stdout.push_str(&format!(" {:02x}", b));
                }
                stdout.push('\n');
                offset += 16;
            }
            if !no_address && !bytes.is_empty() {
                stdout.push_str(&format!("{:07o}\n", bytes.len()));
            }
        } else {
            // Default: octal dump (simplified)
            let mut offset = 0;
            while offset < bytes.len() {
                if !no_address {
                    stdout.push_str(&format!("{:07o}", offset));
                }
                let end = std::cmp::min(offset + 16, bytes.len());
                for b in &bytes[offset..end] {
                    stdout.push_str(&format!(" {:03o}", b));
                }
                stdout.push('\n');
                offset += 16;
            }
            if !no_address && !bytes.is_empty() {
                stdout.push_str(&format!("{:07o}\n", bytes.len()));
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

static WC_META: CommandMeta = CommandMeta {
    name: "wc",
    synopsis: "wc [-lwc] [FILE ...]",
    description: "Print newline, word, and byte counts for each file.",
    options: &[
        ("-l", "print the newline count"),
        ("-w", "print the word count"),
        ("-c", "print the byte count"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for WcCommand {
    fn name(&self) -> &str {
        "wc"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&WC_META)
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
            if !opts_done && arg.starts_with("--") {
                return super::unknown_option("wc", arg);
            } else if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    match c {
                        'l' => show_lines = true,
                        'w' => show_words = true,
                        'c' => show_bytes = true,
                        _ => {
                            return super::unknown_option("wc", &format!("-{}", c));
                        }
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

        // First pass: compute counts for all inputs.
        let mut counts: Vec<(usize, usize, usize)> = Vec::new();
        let mut total_lines = 0usize;
        let mut total_words = 0usize;
        let mut total_bytes = 0usize;

        for (_filename, content) in &inputs {
            let line_count = content.lines().count();
            let word_count = content.split_whitespace().count();
            let byte_count = content.len();
            total_lines += line_count;
            total_words += word_count;
            total_bytes += byte_count;
            counts.push((line_count, word_count, byte_count));
        }

        // Compute dynamic field width like GNU wc: width of the largest number
        // that will appear (including the totals row if present).
        let max_val = {
            let mut m = 0usize;
            let use_totals = inputs.len() > 1;
            for &(l, w, b) in &counts {
                if show_lines {
                    m = m.max(l);
                }
                if show_words {
                    m = m.max(w);
                }
                if show_bytes {
                    m = m.max(b);
                }
            }
            if use_totals {
                if show_lines {
                    m = m.max(total_lines);
                }
                if show_words {
                    m = m.max(total_words);
                }
                if show_bytes {
                    m = m.max(total_bytes);
                }
            }
            m
        };
        let width = if max_val == 0 {
            1
        } else {
            max_val.to_string().len()
        };

        let mut stdout = String::new();

        for (i, (filename, _content)) in inputs.iter().enumerate() {
            let (line_count, word_count, byte_count) = counts[i];

            let mut parts = Vec::new();
            if show_lines {
                parts.push(format!("{:>w$}", line_count, w = width));
            }
            if show_words {
                parts.push(format!("{:>w$}", word_count, w = width));
            }
            if show_bytes {
                parts.push(format!("{:>w$}", byte_count, w = width));
            }

            let display_name = if files.is_empty() {
                String::new()
            } else {
                format!(" {}", filename)
            };
            stdout.push_str(&format!("{}{}\n", parts.join(" "), display_name));
        }

        if inputs.len() > 1 {
            let mut parts = Vec::new();
            if show_lines {
                parts.push(format!("{:>w$}", total_lines, w = width));
            }
            if show_words {
                parts.push(format!("{:>w$}", total_words, w = width));
            }
            if show_bytes {
                parts.push(format!("{:>w$}", total_bytes, w = width));
            }
            stdout.push_str(&format!("{} total\n", parts.join(" ")));
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── tr ───────────────────────────────────────────────────────────────

pub struct TrCommand;

static TR_META: CommandMeta = CommandMeta {
    name: "tr",
    synopsis: "tr [-ds] SET1 [SET2]",
    description: "Translate or delete characters.",
    options: &[
        ("-d", "delete characters in SET1"),
        ("-s", "squeeze repeated output characters"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for TrCommand {
    fn name(&self) -> &str {
        "tr"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TR_META)
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

static REV_META: CommandMeta = CommandMeta {
    name: "rev",
    synopsis: "rev [FILE ...]",
    description: "Reverse lines characterwise.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for RevCommand {
    fn name(&self) -> &str {
        "rev"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&REV_META)
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

static FOLD_META: CommandMeta = CommandMeta {
    name: "fold",
    synopsis: "fold [-s] [-w WIDTH] [FILE ...]",
    description: "Wrap each input line to fit in specified width.",
    options: &[
        ("-w WIDTH", "use WIDTH columns instead of 80"),
        ("-s", "break at spaces"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for FoldCommand {
    fn name(&self) -> &str {
        "fold"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&FOLD_META)
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

static NL_META: CommandMeta = CommandMeta {
    name: "nl",
    synopsis: "nl [FILE ...]",
    description: "Number lines of files.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for NlCommand {
    fn name(&self) -> &str {
        "nl"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&NL_META)
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

static PRINTF_CMD_META: CommandMeta = CommandMeta {
    name: "printf",
    synopsis: "printf [-v var] FORMAT [ARGUMENT ...]",
    description: "Format and print data.",
    options: &[("-v VAR", "assign the output to shell variable VAR")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for PrintfCommand {
    fn name(&self) -> &str {
        "printf"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&PRINTF_CMD_META)
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
        let stdout = run_printf_format(format_str, arguments);

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

/// Run printf formatting with argument cycling (shared between command and builtin).
pub(crate) fn run_printf_format(format_str: &str, arguments: &[String]) -> String {
    let mut stdout = String::new();
    let mut arg_idx = 0;
    let arg_count = arguments.len();

    let need_cycle = arg_count > 0;
    let mut first_pass = true;

    loop {
        let start_arg_idx = arg_idx;
        let (result, terminate) = format_printf(format_str, arguments, &mut arg_idx);
        stdout.push_str(&result);

        if terminate || !need_cycle || arg_idx >= arg_count {
            break;
        }
        if !first_pass && arg_idx == start_arg_idx {
            break;
        }
        first_pass = false;
    }
    stdout
}

pub(crate) fn format_printf(fmt: &str, args: &[String], arg_idx: &mut usize) -> (String, bool) {
    let mut result = String::new();
    let mut terminate = false;
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
            if chars[i] == '%' {
                result.push('%');
                i += 1;
                continue;
            }

            // Parse format specifier: %[flags][width][.precision]conversion
            let mut flags = String::new();
            while i < chars.len() && matches!(chars[i], '-' | '+' | ' ' | '#' | '0') {
                flags.push(chars[i]);
                i += 1;
            }

            // Parse optional width (digits or *)
            let mut width: Option<usize> = None;
            if i < chars.len() && chars[i] == '*' {
                if *arg_idx < args.len() {
                    width = Some(args[*arg_idx].parse::<usize>().unwrap_or(0));
                    *arg_idx += 1;
                }
                i += 1;
            } else {
                let start = i;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                if i > start {
                    let w: String = chars[start..i].iter().collect();
                    width = w.parse().ok();
                }
            }

            // Parse optional precision
            let mut precision: Option<usize> = None;
            if i < chars.len() && chars[i] == '.' {
                i += 1;
                if i < chars.len() && chars[i] == '*' {
                    if *arg_idx < args.len() {
                        precision = Some(args[*arg_idx].parse::<usize>().unwrap_or(0));
                        *arg_idx += 1;
                    }
                    i += 1;
                } else {
                    let start = i;
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        i += 1;
                    }
                    let p: String = if i > start {
                        chars[start..i].iter().collect()
                    } else {
                        "0".to_string()
                    };
                    precision = p.parse().ok();
                }
            }

            if i >= chars.len() {
                result.push('%');
                result.push_str(&flags);
                if let Some(w) = width {
                    result.push_str(&w.to_string());
                }
                if let Some(p) = precision {
                    result.push('.');
                    result.push_str(&p.to_string());
                }
                continue;
            }

            let conv = chars[i];
            let left_align = flags.contains('-');
            let zero_pad = flags.contains('0') && !left_align;
            let plus_sign = flags.contains('+');
            let space_sign = flags.contains(' ') && !plus_sign;
            let alt_form = flags.contains('#');

            match conv {
                's' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.clone()
                    } else {
                        String::new()
                    };
                    let truncated = if let Some(p) = precision {
                        arg.chars().take(p).collect::<String>()
                    } else {
                        arg
                    };
                    let w = width.unwrap_or(0);
                    if left_align {
                        result.push_str(&format!("{:<width$}", truncated, width = w));
                    } else {
                        result.push_str(&format!("{:>width$}", truncated, width = w));
                    }
                }
                'd' | 'i' => {
                    let val = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<i64>().unwrap_or(0)
                    } else {
                        0
                    };
                    let digits = val.unsigned_abs().to_string();
                    let formatted = format_int_padded(&IntFmtOpts {
                        prefix: "",
                        digits: &digits,
                        negative: val < 0,
                        zero_pad,
                        left_align,
                        plus_sign,
                        space_sign,
                        width,
                    });
                    result.push_str(&formatted);
                }
                'o' => {
                    let val = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<i64>().unwrap_or(0)
                    } else {
                        0
                    };
                    let prefix = if alt_form && val != 0 { "0" } else { "" };
                    let digits = format!("{:o}", val.unsigned_abs());
                    let formatted = format_int_padded(&IntFmtOpts {
                        prefix,
                        digits: &digits,
                        negative: val < 0,
                        zero_pad,
                        left_align,
                        plus_sign,
                        space_sign,
                        width,
                    });
                    result.push_str(&formatted);
                }
                'x' | 'X' => {
                    let val = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<i64>().unwrap_or(0)
                    } else {
                        0
                    };
                    let prefix = if alt_form && val != 0 {
                        if conv == 'x' { "0x" } else { "0X" }
                    } else {
                        ""
                    };
                    let digits = if conv == 'x' {
                        format!("{:x}", val.unsigned_abs())
                    } else {
                        format!("{:X}", val.unsigned_abs())
                    };
                    let formatted = format_int_padded(&IntFmtOpts {
                        prefix,
                        digits: &digits,
                        negative: val < 0,
                        zero_pad,
                        left_align,
                        plus_sign,
                        space_sign,
                        width,
                    });
                    result.push_str(&formatted);
                }
                'f' | 'e' | 'E' | 'g' | 'G' => {
                    let val = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.parse::<f64>().unwrap_or(0.0)
                    } else {
                        0.0
                    };
                    let prec = precision.unwrap_or(6);
                    let num_str = match conv {
                        'e' => format!("{:.prec$e}", val),
                        'E' => format!("{:.prec$E}", val),
                        'g' | 'G' => {
                            // %g uses shorter of %e or %f
                            let f_str = format!("{:.prec$}", val);
                            let e_str = if conv == 'g' {
                                format!("{:.prec$e}", val)
                            } else {
                                format!("{:.prec$E}", val)
                            };
                            if e_str.len() < f_str.len() {
                                e_str
                            } else {
                                f_str
                            }
                        }
                        _ => format!("{:.prec$}", val),
                    };
                    let sign = if val.is_sign_negative() && !num_str.starts_with('-') {
                        "-"
                    } else if plus_sign && !num_str.starts_with('-') {
                        "+"
                    } else if space_sign && !num_str.starts_with('-') {
                        " "
                    } else {
                        ""
                    };
                    let full = format!("{sign}{num_str}");
                    let w = width.unwrap_or(0);
                    if left_align {
                        result.push_str(&format!("{:<width$}", full, width = w));
                    } else if zero_pad {
                        // For zero-padded floats, pad between sign and digits
                        if full.starts_with('-') || full.starts_with('+') || full.starts_with(' ') {
                            let (s, rest) = full.split_at(1);
                            if w > full.len() {
                                let pad = "0".repeat(w - full.len());
                                result.push_str(s);
                                result.push_str(&pad);
                                result.push_str(rest);
                            } else {
                                result.push_str(&full);
                            }
                        } else {
                            result.push_str(&format!("{:0>width$}", full, width = w));
                        }
                    } else {
                        result.push_str(&format!("{:>width$}", full, width = w));
                    }
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
                'b' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.clone()
                    } else {
                        String::new()
                    };
                    let (expanded, should_terminate) = expand_printf_backslash_escapes(&arg);
                    result.push_str(&expanded);
                    if should_terminate {
                        terminate = true;
                        break;
                    }
                }
                'q' => {
                    let arg = if *arg_idx < args.len() {
                        let a = &args[*arg_idx];
                        *arg_idx += 1;
                        a.clone()
                    } else {
                        String::new()
                    };
                    result.push_str(&printf_shell_quote(&arg));
                }
                _ => {
                    result.push('%');
                    result.push_str(&flags);
                    if let Some(w) = width {
                        result.push_str(&w.to_string());
                    }
                    if let Some(p) = precision {
                        result.push('.');
                        result.push_str(&p.to_string());
                    }
                    result.push(conv);
                }
            }
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }
    (result, terminate)
}

struct IntFmtOpts<'a> {
    prefix: &'a str,
    digits: &'a str,
    negative: bool,
    zero_pad: bool,
    left_align: bool,
    plus_sign: bool,
    space_sign: bool,
    width: Option<usize>,
}

fn format_int_padded(opts: &IntFmtOpts) -> String {
    let sign = if opts.negative {
        "-"
    } else if opts.plus_sign {
        "+"
    } else if opts.space_sign {
        " "
    } else {
        ""
    };

    let core_len = sign.len() + opts.prefix.len() + opts.digits.len();
    let w = opts.width.unwrap_or(0);

    if opts.left_align {
        let mut s = format!("{}{}{}", sign, opts.prefix, opts.digits);
        while s.len() < w {
            s.push(' ');
        }
        s
    } else if opts.zero_pad {
        let pad = w.saturating_sub(core_len);
        format!("{}{}{}{}", sign, opts.prefix, "0".repeat(pad), opts.digits)
    } else {
        let full = format!("{}{}{}", sign, opts.prefix, opts.digits);
        if w > full.len() {
            format!("{}{full}", " ".repeat(w - full.len()))
        } else {
            full
        }
    }
}

/// Expand backslash escapes for printf %b format specifier.
/// Returns `(expanded_text, should_terminate)`. `should_terminate` is true if `\c` was encountered.
fn expand_printf_backslash_escapes(s: &str) -> (String, bool) {
    let mut result = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            i += 1;
            match chars[i] {
                'c' => return (result, true),
                'n' => result.push('\n'),
                't' => result.push('\t'),
                'r' => result.push('\r'),
                '\\' => result.push('\\'),
                'a' => result.push('\x07'),
                'b' => result.push('\x08'),
                'f' => result.push('\x0C'),
                'v' => result.push('\x0B'),
                '0' => {
                    let mut val = 0u32;
                    let mut count = 0;
                    while count < 3
                        && i + 1 < chars.len()
                        && chars[i + 1] >= '0'
                        && chars[i + 1] <= '7'
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
        } else {
            result.push(chars[i]);
        }
        i += 1;
    }
    (result, false)
}

/// Shell-quote a string for printf %q.
fn printf_shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    // If string contains control characters, use $'...' notation
    let has_control = s.chars().any(|c| c.is_ascii_control());
    if has_control {
        let mut out = String::from("$'");
        for ch in s.chars() {
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
        return out;
    }
    // Check if the string needs quoting at all
    let needs_quoting = s
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && !"@%_+:,./=-".contains(c));
    if !needs_quoting {
        return s.to_string();
    }
    // Use backslash escaping (bash's default %q behavior)
    let mut result = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || "@%_+:,./=-".contains(ch) {
            result.push(ch);
        } else {
            result.push('\\');
            result.push(ch);
        }
    }
    result
}

// ── paste ────────────────────────────────────────────────────────────

pub struct PasteCommand;

static PASTE_META: CommandMeta = CommandMeta {
    name: "paste",
    synopsis: "paste [-d DELIM] [FILE ...]",
    description: "Merge lines of files.",
    options: &[("-d DELIM", "use DELIM instead of TAB as delimiter")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for PasteCommand {
    fn name(&self) -> &str {
        "paste"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&PASTE_META)
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

// ── tac ──────────────────────────────────────────────────────────────

pub struct TacCommand;

static TAC_META: CommandMeta = CommandMeta {
    name: "tac",
    synopsis: "tac [-s SEP] [FILE ...]",
    description: "Concatenate and print files in reverse.",
    options: &[("-s SEP", "use SEP as the record separator")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for TacCommand {
    fn name(&self) -> &str {
        "tac"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TAC_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut separator: Option<&str> = None;
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
            if !opts_done && arg == "-s" {
                i += 1;
                if i < args.len() {
                    separator = Some(&args[i]);
                }
            } else if !opts_done && arg.starts_with("-s") {
                separator = Some(&arg[2..]);
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

        let sep = separator.unwrap_or("\n");
        let mut parts: Vec<&str> = content.split(sep).collect();
        // If content ends with the separator, the last element is empty — remove it
        // so we don't produce a leading separator in the output.
        if parts.last() == Some(&"") {
            parts.pop();
        }
        parts.reverse();

        let mut stdout = String::new();
        if !parts.is_empty() {
            for (idx, part) in parts.iter().enumerate() {
                stdout.push_str(part);
                if idx < parts.len() - 1 {
                    stdout.push_str(sep);
                }
            }
            stdout.push('\n');
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

// ── comm ─────────────────────────────────────────────────────────────

pub struct CommCommand;

static COMM_META: CommandMeta = CommandMeta {
    name: "comm",
    synopsis: "comm [-123] FILE1 FILE2",
    description: "Compare two sorted files line by line.",
    options: &[
        ("-1", "suppress column 1 (lines unique to FILE1)"),
        ("-2", "suppress column 2 (lines unique to FILE2)"),
        ("-3", "suppress column 3 (lines common to both)"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for CommCommand {
    fn name(&self) -> &str {
        "comm"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&COMM_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut suppress1 = false;
        let mut suppress2 = false;
        let mut suppress3 = false;
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
            if !opts_done && arg.starts_with('-') && arg != "-" {
                if arg == "--check-order" {
                    // default behavior, no-op
                } else {
                    for c in arg[1..].chars() {
                        match c {
                            '1' => suppress1 = true,
                            '2' => suppress2 = true,
                            '3' => suppress3 = true,
                            _ => {
                                return CommandResult {
                                    stderr: format!("comm: invalid option -- '{}'\n", c),
                                    exit_code: 1,
                                    ..Default::default()
                                };
                            }
                        }
                    }
                }
            } else {
                files.push(arg);
            }
            i += 1;
        }

        if files.len() != 2 {
            return CommandResult {
                stderr: "comm: requires exactly two file arguments\n".to_string(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let inputs = match read_input(&files, ctx) {
            Ok(i) => i,
            Err(r) => return r,
        };

        let lines1: Vec<&str> = inputs[0].1.lines().collect();
        let lines2: Vec<&str> = inputs[1].1.lines().collect();

        let mut stdout = String::new();
        let mut i1 = 0;
        let mut i2 = 0;

        while i1 < lines1.len() && i2 < lines2.len() {
            match lines1[i1].cmp(lines2[i2]) {
                std::cmp::Ordering::Less => {
                    if !suppress1 {
                        stdout.push_str(lines1[i1]);
                        stdout.push('\n');
                    }
                    i1 += 1;
                }
                std::cmp::Ordering::Greater => {
                    if !suppress2 {
                        if !suppress1 {
                            stdout.push('\t');
                        }
                        stdout.push_str(lines2[i2]);
                        stdout.push('\n');
                    }
                    i2 += 1;
                }
                std::cmp::Ordering::Equal => {
                    if !suppress3 {
                        if !suppress1 {
                            stdout.push('\t');
                        }
                        if !suppress2 {
                            stdout.push('\t');
                        }
                        stdout.push_str(lines1[i1]);
                        stdout.push('\n');
                    }
                    i1 += 1;
                    i2 += 1;
                }
            }
        }

        while i1 < lines1.len() {
            if !suppress1 {
                stdout.push_str(lines1[i1]);
                stdout.push('\n');
            }
            i1 += 1;
        }

        while i2 < lines2.len() {
            if !suppress2 {
                if !suppress1 {
                    stdout.push('\t');
                }
                stdout.push_str(lines2[i2]);
                stdout.push('\n');
            }
            i2 += 1;
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── join ─────────────────────────────────────────────────────────────

pub struct JoinCommand;

static JOIN_META: CommandMeta = CommandMeta {
    name: "join",
    synopsis: "join [-t SEP] [-1 FIELD] [-2 FIELD] FILE1 FILE2",
    description: "Join lines of two files on a common field.",
    options: &[
        ("-t SEP", "use SEP as input and output field separator"),
        ("-j FIELD", "equivalent to -1 FIELD -2 FIELD"),
        ("-1 FIELD", "join on this field of file 1"),
        ("-2 FIELD", "join on this field of file 2"),
        ("-a FILENUM", "print unpairable lines from file FILENUM"),
        ("-e STRING", "replace missing input fields with STRING"),
        ("-o FORMAT", "output format specification"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for JoinCommand {
    fn name(&self) -> &str {
        "join"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&JOIN_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut field1: usize = 1;
        let mut field2: usize = 1;
        let mut separator: Option<String> = None;
        let mut unpaired: Vec<usize> = Vec::new();
        let mut empty_replacement: Option<String> = None;
        let mut output_format: Option<String> = None;
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
            if !opts_done && arg == "-t" {
                i += 1;
                if i < args.len() {
                    separator = Some(args[i].clone());
                }
            } else if !opts_done && arg.starts_with("-t") {
                separator = Some(arg[2..].to_string());
            } else if !opts_done && arg == "-j" {
                i += 1;
                if i < args.len() {
                    let f: usize = args[i].parse().unwrap_or(1);
                    field1 = f;
                    field2 = f;
                }
            } else if !opts_done && arg == "-1" {
                i += 1;
                if i < args.len() {
                    field1 = args[i].parse().unwrap_or(1);
                }
            } else if !opts_done && arg == "-2" {
                i += 1;
                if i < args.len() {
                    field2 = args[i].parse().unwrap_or(1);
                }
            } else if !opts_done && arg == "-a" {
                i += 1;
                if i < args.len()
                    && let Ok(n) = args[i].parse::<usize>()
                {
                    unpaired.push(n);
                }
            } else if !opts_done && arg == "-e" {
                i += 1;
                if i < args.len() {
                    empty_replacement = Some(args[i].clone());
                }
            } else if !opts_done && arg == "-o" {
                i += 1;
                if i < args.len() {
                    output_format = Some(args[i].clone());
                }
            } else {
                files.push(arg);
            }
            i += 1;
        }

        if files.len() != 2 {
            return CommandResult {
                stderr: "join: requires exactly two file arguments\n".to_string(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let inputs = match read_input(&files, ctx) {
            Ok(i) => i,
            Err(r) => return r,
        };

        let out_sep = separator.as_deref().unwrap_or(" ");

        let split_line = |line: &str| -> Vec<String> {
            if let Some(ref sep) = separator {
                line.split(sep.as_str()).map(|s| s.to_string()).collect()
            } else {
                line.split_whitespace().map(|s| s.to_string()).collect()
            }
        };

        let get_key = |fields: &[String], field_idx: usize| -> String {
            if field_idx == 0 || field_idx > fields.len() {
                String::new()
            } else {
                fields[field_idx - 1].clone()
            }
        };

        let lines1: Vec<Vec<String>> = inputs[0].1.lines().map(split_line).collect();
        let lines2: Vec<Vec<String>> = inputs[1].1.lines().map(split_line).collect();

        let format_output =
            |key: &str, f1: Option<&Vec<String>>, f2: Option<&Vec<String>>| -> String {
                if let Some(ref fmt) = output_format {
                    let specs: Vec<&str> = fmt.split(',').collect();
                    let mut parts: Vec<String> = Vec::new();
                    for spec in &specs {
                        if *spec == "0" {
                            parts.push(key.to_string());
                        } else if let Some(rest) = spec.strip_prefix("1.")
                            && let Ok(idx) = rest.parse::<usize>()
                        {
                            let val = f1
                                .and_then(|f| {
                                    if idx > 0 && idx <= f.len() {
                                        Some(f[idx - 1].as_str())
                                    } else {
                                        None
                                    }
                                })
                                .or(empty_replacement.as_deref())
                                .unwrap_or("");
                            parts.push(val.to_string());
                        } else if let Some(rest) = spec.strip_prefix("2.")
                            && let Ok(idx) = rest.parse::<usize>()
                        {
                            let val = f2
                                .and_then(|f| {
                                    if idx > 0 && idx <= f.len() {
                                        Some(f[idx - 1].as_str())
                                    } else {
                                        None
                                    }
                                })
                                .or(empty_replacement.as_deref())
                                .unwrap_or("");
                            parts.push(val.to_string());
                        }
                    }
                    parts.join(out_sep)
                } else {
                    let mut parts: Vec<String> = vec![key.to_string()];
                    if let Some(f) = f1 {
                        for (idx, val) in f.iter().enumerate() {
                            if idx + 1 != field1 {
                                parts.push(val.clone());
                            }
                        }
                    }
                    if let Some(f) = f2 {
                        for (idx, val) in f.iter().enumerate() {
                            if idx + 1 != field2 {
                                parts.push(val.clone());
                            }
                        }
                    }
                    parts.join(out_sep)
                }
            };

        let mut stdout = String::new();
        let mut j = 0;
        let mut match_end: usize = 0;
        let mut prev_key1 = String::new();

        for fields1 in &lines1 {
            let key1 = get_key(fields1, field1);

            if key1 != prev_key1 {
                j = match_end;

                while j < lines2.len() {
                    let key2 = get_key(&lines2[j], field2);
                    if key2 < key1 {
                        if unpaired.contains(&2) {
                            stdout.push_str(&format_output(&key2, None, Some(&lines2[j])));
                            stdout.push('\n');
                        }
                        j += 1;
                    } else {
                        break;
                    }
                }
            }

            let mut k = j;
            let mut matched = false;
            while k < lines2.len() {
                let key2 = get_key(&lines2[k], field2);
                if key2 == key1 {
                    stdout.push_str(&format_output(&key1, Some(fields1), Some(&lines2[k])));
                    stdout.push('\n');
                    matched = true;
                    k += 1;
                } else {
                    break;
                }
            }
            match_end = match_end.max(k);

            if !matched && unpaired.contains(&1) {
                stdout.push_str(&format_output(&key1, Some(fields1), None));
                stdout.push('\n');
            }
            prev_key1 = key1;
        }

        // Remaining unmatched lines from file 2
        j = match_end;
        while j < lines2.len() {
            if unpaired.contains(&2) {
                stdout.push_str(&format_output(
                    &get_key(&lines2[j], field2),
                    None,
                    Some(&lines2[j]),
                ));
                stdout.push('\n');
            }
            j += 1;
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── fmt ──────────────────────────────────────────────────────────────

pub struct FmtCommand;

static FMT_META: CommandMeta = CommandMeta {
    name: "fmt",
    synopsis: "fmt [-s] [-w WIDTH] [FILE ...]",
    description: "Simple optimal text formatter.",
    options: &[
        ("-w WIDTH", "maximum line width (default 75)"),
        ("-s", "split long lines only, do not refill"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for FmtCommand {
    fn name(&self) -> &str {
        "fmt"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&FMT_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut width: usize = 75;
        let mut split_only = false;
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
                    width = args[i].parse().unwrap_or(75).max(1);
                }
            } else if !opts_done && arg.starts_with("-w") {
                width = arg[2..].parse().unwrap_or(75).max(1);
            } else if !opts_done && arg == "-s" {
                split_only = true;
            } else if !opts_done && arg.starts_with('-') && arg != "-" {
                // ignore unknown options
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
        let paragraphs = split_paragraphs(&content);

        for para in &paragraphs {
            if para.is_empty() {
                stdout.push('\n');
                continue;
            }

            if split_only {
                for line in para.lines() {
                    if line.len() <= width {
                        stdout.push_str(line);
                        stdout.push('\n');
                    } else {
                        let mut remaining = line;
                        while remaining.len() > width {
                            let split_pos = remaining[..width].rfind(' ').unwrap_or(width);
                            stdout.push_str(&remaining[..split_pos]);
                            stdout.push('\n');
                            remaining = remaining[split_pos..].trim_start();
                        }
                        if !remaining.is_empty() {
                            stdout.push_str(remaining);
                            stdout.push('\n');
                        }
                    }
                }
            } else {
                let words: Vec<&str> = para.split_whitespace().collect();
                let mut line_len = 0;
                for (word_idx, word) in words.iter().enumerate() {
                    if word_idx == 0 {
                        stdout.push_str(word);
                        line_len = word.len();
                    } else if line_len + 1 + word.len() > width {
                        stdout.push('\n');
                        stdout.push_str(word);
                        line_len = word.len();
                    } else {
                        stdout.push(' ');
                        stdout.push_str(word);
                        line_len += 1 + word.len();
                    }
                }
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

fn split_paragraphs(input: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();

    for line in input.lines() {
        if line.trim().is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.clone());
                current.clear();
            }
            paragraphs.push(String::new());
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line);
        }
    }

    if !current.is_empty() {
        paragraphs.push(current);
    }

    paragraphs
}

// ── column ───────────────────────────────────────────────────────────

pub struct ColumnCommand;

static COLUMN_META: CommandMeta = CommandMeta {
    name: "column",
    synopsis: "column [-t] [-s SEP] [-o SEP] [FILE ...]",
    description: "Columnate lists.",
    options: &[
        ("-t", "create a table"),
        ("-s SEP", "specify input column separator"),
        ("-o SEP", "specify output column separator"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for ColumnCommand {
    fn name(&self) -> &str {
        "column"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&COLUMN_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut table_mode = false;
        let mut input_sep: Option<String> = None;
        let mut output_sep = "  ".to_string();
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
            if !opts_done && arg == "-t" {
                table_mode = true;
            } else if !opts_done && arg == "-s" {
                i += 1;
                if i < args.len() {
                    input_sep = Some(args[i].clone());
                }
            } else if !opts_done && arg.starts_with("-s") {
                input_sep = Some(arg[2..].to_string());
            } else if !opts_done && arg == "-o" {
                i += 1;
                if i < args.len() {
                    output_sep = args[i].clone();
                }
            } else if !opts_done && arg.starts_with("-o") {
                output_sep = arg[2..].to_string();
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

        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            return CommandResult::default();
        }

        if table_mode {
            let split_line = |line: &str| -> Vec<String> {
                if let Some(ref sep) = input_sep {
                    line.split(sep.as_str())
                        .map(|s| s.trim().to_string())
                        .collect()
                } else {
                    line.split_whitespace().map(|s| s.to_string()).collect()
                }
            };

            let rows: Vec<Vec<String>> = lines.iter().map(|l| split_line(l)).collect();
            let num_cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
            let mut col_widths = vec![0usize; num_cols];
            for row in &rows {
                for (col_idx, cell) in row.iter().enumerate() {
                    col_widths[col_idx] = col_widths[col_idx].max(cell.len());
                }
            }

            let mut stdout = String::new();
            for row in &rows {
                for (col_idx, cell) in row.iter().enumerate() {
                    if col_idx > 0 {
                        stdout.push_str(&output_sep);
                    }
                    if col_idx < row.len() - 1 {
                        stdout.push_str(cell);
                        let padding = col_widths[col_idx].saturating_sub(cell.len());
                        for _ in 0..padding {
                            stdout.push(' ');
                        }
                    } else {
                        stdout.push_str(cell);
                    }
                }
                stdout.push('\n');
            }

            CommandResult {
                stdout,
                stderr,
                exit_code: 0,
            }
        } else {
            // Fill columns mode (newspaper style)
            let max_width = 80;
            let max_len = lines.iter().map(|l| l.len()).max().unwrap_or(0);
            let col_width = max_len + 2;
            let num_cols = (max_width / col_width).max(1);
            let num_rows = lines.len().div_ceil(num_cols);

            let mut stdout = String::new();
            for row in 0..num_rows {
                for col in 0..num_cols {
                    let idx = col * num_rows + row;
                    if idx < lines.len() {
                        if col > 0 {
                            stdout.push_str("  ");
                        }
                        let entry = lines[idx];
                        stdout.push_str(entry);
                        if col < num_cols - 1 && (col + 1) * num_rows + row < lines.len() {
                            let padding = col_width.saturating_sub(entry.len() + 2);
                            for _ in 0..padding {
                                stdout.push(' ');
                            }
                        }
                    }
                }
                stdout.push('\n');
            }

            CommandResult {
                stdout,
                stderr,
                exit_code: 0,
            }
        }
    }
}

// ── expand ───────────────────────────────────────────────────────────

pub struct ExpandCommand;

static EXPAND_META: CommandMeta = CommandMeta {
    name: "expand",
    synopsis: "expand [-t STOPS] [FILE ...]",
    description: "Convert tabs to spaces.",
    options: &[("-t STOPS", "set tab stops")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for ExpandCommand {
    fn name(&self) -> &str {
        "expand"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&EXPAND_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut tab_stops = TabStops::Uniform(8);
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
            if !opts_done && arg == "-t" {
                i += 1;
                if i < args.len() {
                    tab_stops = parse_tab_stops(&args[i]);
                }
            } else if !opts_done && arg.starts_with("-t") {
                tab_stops = parse_tab_stops(&arg[2..]);
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
            let mut col = 0;
            for ch in line.chars() {
                if ch == '\t' {
                    let next_stop = next_tab_stop(col, &tab_stops);
                    let spaces = next_stop - col;
                    for _ in 0..spaces {
                        stdout.push(' ');
                    }
                    col = next_stop;
                } else {
                    stdout.push(ch);
                    col += 1;
                }
            }
            stdout.push('\n');
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

// ── unexpand ─────────────────────────────────────────────────────────

pub struct UnexpandCommand;

static UNEXPAND_META: CommandMeta = CommandMeta {
    name: "unexpand",
    synopsis: "unexpand [-a] [-t NUM] [FILE ...]",
    description: "Convert spaces to tabs.",
    options: &[
        ("-a", "convert all blanks, not just leading"),
        ("-t NUM", "set tab width (default 8)"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for UnexpandCommand {
    fn name(&self) -> &str {
        "unexpand"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&UNEXPAND_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut tab_width: usize = 8;
        let mut convert_all = false;
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
            if !opts_done && arg == "-a" {
                convert_all = true;
            } else if !opts_done && arg == "-t" {
                i += 1;
                if i < args.len() {
                    tab_width = args[i].parse().unwrap_or(8);
                    convert_all = true;
                }
            } else if !opts_done && arg.starts_with("-t") {
                tab_width = arg[2..].parse().unwrap_or(8);
                convert_all = true;
            } else {
                files.push(arg);
            }
            i += 1;
        }

        if tab_width == 0 {
            tab_width = 8;
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
            stdout.push_str(&unexpand_line(line, tab_width, convert_all));
            stdout.push('\n');
        }

        CommandResult {
            stdout,
            stderr,
            exit_code: 0,
        }
    }
}

enum TabStops {
    Uniform(usize),
    List(Vec<usize>),
}

fn parse_tab_stops(s: &str) -> TabStops {
    if s.contains(',') {
        let stops: Vec<usize> = s.split(',').filter_map(|p| p.trim().parse().ok()).collect();
        if stops.is_empty() {
            TabStops::Uniform(8)
        } else {
            TabStops::List(stops)
        }
    } else {
        match s.parse::<usize>() {
            Ok(n) if n > 0 => TabStops::Uniform(n),
            _ => TabStops::Uniform(8),
        }
    }
}

fn next_tab_stop(col: usize, stops: &TabStops) -> usize {
    match stops {
        TabStops::Uniform(n) => ((col / n) + 1) * n,
        TabStops::List(list) => {
            for &stop in list {
                if stop > col {
                    return stop;
                }
            }
            // Past all explicit stops, use last interval or just advance by 1
            if let Some(&last) = list.last() {
                let interval = if list.len() >= 2 {
                    last - list[list.len() - 2]
                } else {
                    last
                };
                let past = col - last;
                last + ((past / interval) + 1) * interval
            } else {
                col + 1
            }
        }
    }
}

fn unexpand_line(line: &str, tab_width: usize, convert_all: bool) -> String {
    if !convert_all {
        // Only convert leading spaces
        let mut result = String::new();
        let mut space_count = 0;
        let mut in_leading = true;

        for ch in line.chars() {
            if in_leading && ch == ' ' {
                space_count += 1;
                if space_count == tab_width {
                    result.push('\t');
                    space_count = 0;
                }
            } else {
                if in_leading {
                    for _ in 0..space_count {
                        result.push(' ');
                    }
                    space_count = 0;
                    in_leading = false;
                }
                result.push(ch);
            }
        }
        // Handle trailing leading spaces
        for _ in 0..space_count {
            result.push(' ');
        }
        result
    } else {
        // Convert all sequences of spaces at tab boundaries
        let mut result = String::new();
        let mut col = 0;
        let mut space_start_col = None;

        for ch in line.chars() {
            if ch == ' ' {
                if space_start_col.is_none() {
                    space_start_col = Some(col);
                }
                col += 1;
                // Check if we're at a tab stop
                if col % tab_width == 0 {
                    result.push('\t');
                    space_start_col = None;
                }
            } else {
                if let Some(start) = space_start_col {
                    // Flush remaining spaces that didn't reach a tab stop
                    let spaces = col % tab_width;
                    if spaces > 0 {
                        let start_in_tab = start % tab_width;
                        for _ in start_in_tab..(start_in_tab + (col - start)) {
                            result.push(' ');
                        }
                    }
                    space_start_col = None;
                }
                result.push(ch);
                col += 1;
            }
        }
        // Flush any trailing spaces
        if let Some(start) = space_start_col {
            for _ in 0..(col - start) {
                result.push(' ');
            }
        }
        result
    }
}

// ── strings ─────────────────────────────────────────────────────────

pub struct StringsCommand;

static STRINGS_META: CommandMeta = CommandMeta {
    name: "strings",
    synopsis: "strings [-n MIN] [FILE ...]",
    description: "Print the sequences of printable characters in files.",
    options: &[("-n MIN", "set minimum string length (default 4)")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for StringsCommand {
    fn name(&self) -> &str {
        "strings"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&STRINGS_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut min_len: usize = 4;
        let mut files: Vec<&str> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            if arg == "-n" {
                i += 1;
                if i < args.len() {
                    min_len = args[i].parse().unwrap_or(4);
                }
            } else if let Some(v) = arg.strip_prefix("-n") {
                min_len = v.parse().unwrap_or(4);
            } else if arg == "-a" || arg == "--" {
                // -a is default behavior, skip
            } else {
                files.push(arg);
            }
            i += 1;
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        let sources: Vec<(&str, Vec<u8>)> = if files.is_empty() {
            vec![("-", ctx.stdin.as_bytes().to_vec())]
        } else {
            let mut v = Vec::new();
            for f in &files {
                if *f == "-" {
                    v.push(("-", ctx.stdin.as_bytes().to_vec()));
                } else {
                    let path = resolve_path(f, ctx.cwd);
                    match ctx.fs.read_file(&path) {
                        Ok(data) => v.push((*f, data)),
                        Err(e) => {
                            stderr.push_str(&format!("strings: {}: {}\n", f, e));
                            exit_code = 1;
                        }
                    }
                }
            }
            v
        };

        for (_name, data) in &sources {
            let mut run = String::new();
            for &byte in data.iter() {
                if (0x20..0x7f).contains(&byte) {
                    run.push(byte as char);
                } else {
                    if run.len() >= min_len {
                        stdout.push_str(&run);
                        stdout.push('\n');
                    }
                    run.clear();
                }
            }
            if run.len() >= min_len {
                stdout.push_str(&run);
                stdout.push('\n');
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code,
        }
    }
}

// ── rg (ripgrep) ────────────────────────────────────────────────────

pub struct RgCommand;

static RG_META: CommandMeta = CommandMeta {
    name: "rg",
    synopsis: "rg [OPTIONS] PATTERN [PATH ...]",
    description: "Recursively search for a pattern in files.",
    options: &[
        ("-i, --ignore-case", "case insensitive search"),
        ("-n, --line-number", "show line numbers (default)"),
        ("-l, --files-with-matches", "only show matching file names"),
        ("-c, --count", "show match count per file"),
        ("-w, --word-regexp", "only match whole words"),
        ("-t TYPE", "only search files of TYPE"),
        ("-T TYPE", "exclude files of TYPE"),
        ("-g GLOB", "include or exclude files matching GLOB"),
        ("--vimgrep", "show results in vimgrep format"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for RgCommand {
    fn name(&self) -> &str {
        "rg"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&RG_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut case_insensitive = false;
        let mut show_line_numbers = true;
        let mut files_only = false;
        let mut count_only = false;
        let mut word_regexp = false;
        let mut vimgrep = false;
        let mut type_includes: Vec<&str> = Vec::new();
        let mut type_excludes: Vec<&str> = Vec::new();
        let mut globs: Vec<&str> = Vec::new();
        let mut pattern: Option<&str> = None;
        let mut paths: Vec<&str> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            match arg.as_str() {
                "-i" | "--ignore-case" => case_insensitive = true,
                "-n" | "--line-number" => show_line_numbers = true,
                "-l" | "--files-with-matches" => files_only = true,
                "-c" | "--count" => count_only = true,
                "-w" | "--word-regexp" => word_regexp = true,
                "--vimgrep" => vimgrep = true,
                "--no-line-number" | "-N" => show_line_numbers = false,
                "-t" => {
                    i += 1;
                    if i < args.len() {
                        type_includes.push(&args[i]);
                    }
                }
                "-T" => {
                    i += 1;
                    if i < args.len() {
                        type_excludes.push(&args[i]);
                    }
                }
                "-g" => {
                    i += 1;
                    if i < args.len() {
                        globs.push(&args[i]);
                    }
                }
                _ if arg.starts_with("--type=") => {
                    type_includes.push(&arg[7..]);
                }
                _ if arg.starts_with("-g") && arg.len() > 2 => {
                    globs.push(&arg[2..]);
                }
                _ if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") => {
                    // Combined short flags
                    for c in arg[1..].chars() {
                        match c {
                            'i' => case_insensitive = true,
                            'n' => show_line_numbers = true,
                            'l' => files_only = true,
                            'c' => count_only = true,
                            'w' => word_regexp = true,
                            _ => {}
                        }
                    }
                }
                _ if pattern.is_none() => pattern = Some(arg),
                _ => paths.push(arg),
            }
            i += 1;
        }

        let pattern = match pattern {
            Some(p) => p,
            None => {
                return CommandResult {
                    stderr: "rg: no pattern given\n".into(),
                    exit_code: 2,
                    ..Default::default()
                };
            }
        };

        // Build regex
        let mut pat = if word_regexp {
            format!(r"\b{}\b", pattern)
        } else {
            pattern.to_string()
        };

        if case_insensitive {
            pat = format!("(?i){}", pat);
        }

        let re = match Regex::new(&pat) {
            Ok(r) => r,
            Err(e) => {
                return CommandResult {
                    stderr: format!("rg: regex error: {}\n", e),
                    exit_code: 2,
                    ..Default::default()
                };
            }
        };

        // If no paths, search cwd recursively
        if paths.is_empty() {
            paths.push(".");
        }

        // Load .gitignore patterns from VFS
        let gitignore_patterns = load_gitignore_patterns(ctx);

        // Collect files to search
        let mut file_contents: Vec<(String, String)> = Vec::new();
        for p in &paths {
            let path = resolve_path(p, ctx.cwd);
            match ctx.fs.stat(&path) {
                Ok(meta) if meta.node_type == crate::vfs::NodeType::Directory => {
                    rg_collect_dir(
                        &path,
                        ctx,
                        &type_includes,
                        &type_excludes,
                        &globs,
                        &gitignore_patterns,
                        &mut file_contents,
                    );
                }
                Ok(_) => {
                    if let Ok(bytes) = ctx.fs.read_file(&path) {
                        file_contents
                            .push((p.to_string(), String::from_utf8_lossy(&bytes).to_string()));
                    }
                }
                Err(_) => {}
            }
        }

        file_contents.sort_by(|a, b| a.0.cmp(&b.0));

        let mut stdout = String::new();
        let mut any_match = false;

        for (filename, content) in &file_contents {
            let mut file_match_count = 0usize;

            for (line_idx, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    file_match_count += 1;
                    any_match = true;

                    if !count_only && !files_only {
                        if vimgrep {
                            // vimgrep format: file:line:col:text
                            if let Some(m) = re.find(line) {
                                stdout.push_str(&format!(
                                    "{}:{}:{}:{}\n",
                                    filename,
                                    line_idx + 1,
                                    m.start() + 1,
                                    line
                                ));
                            }
                        } else if show_line_numbers {
                            stdout.push_str(&format!("{}:{}:{}\n", filename, line_idx + 1, line));
                        } else {
                            stdout.push_str(&format!("{}:{}\n", filename, line));
                        }
                    }
                }
            }

            if count_only && file_match_count > 0 {
                stdout.push_str(&format!("{}:{}\n", filename, file_match_count));
            }
            if files_only && file_match_count > 0 {
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

fn rg_type_extensions(type_name: &str) -> &'static [&'static str] {
    match type_name {
        "py" | "python" => &["py"],
        "js" | "javascript" => &["js", "jsx", "mjs"],
        "ts" | "typescript" => &["ts", "tsx"],
        "rs" | "rust" => &["rs"],
        "go" => &["go"],
        "c" => &["c", "h"],
        "cpp" => &["cpp", "cc", "cxx", "hpp", "hxx"],
        "java" => &["java"],
        "rb" | "ruby" => &["rb"],
        "sh" | "shell" => &["sh", "bash"],
        "html" => &["html", "htm"],
        "css" => &["css"],
        "json" => &["json"],
        "yaml" | "yml" => &["yaml", "yml"],
        "xml" => &["xml"],
        "md" | "markdown" => &["md"],
        "txt" => &["txt"],
        "toml" => &["toml"],
        _ => &[],
    }
}

fn rg_file_matches_type(name: &str, types: &[&str]) -> bool {
    if types.is_empty() {
        return true;
    }
    let ext = name.rsplit('.').next().unwrap_or("");
    types.iter().any(|t| rg_type_extensions(t).contains(&ext))
}

fn rg_file_excluded_type(name: &str, types: &[&str]) -> bool {
    if types.is_empty() {
        return false;
    }
    let ext = name.rsplit('.').next().unwrap_or("");
    types.iter().any(|t| rg_type_extensions(t).contains(&ext))
}

fn load_gitignore_patterns(ctx: &CommandContext) -> Vec<String> {
    let mut patterns = Vec::new();
    for gitignore_path in &[
        std::path::PathBuf::from("/.gitignore"),
        std::path::PathBuf::from(ctx.cwd).join(".gitignore"),
    ] {
        if let Ok(bytes) = ctx.fs.read_file(gitignore_path) {
            let content = String::from_utf8_lossy(&bytes);
            for line in content.lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    patterns.push(line.to_string());
                }
            }
        }
    }
    patterns
}

fn rg_is_gitignored(name: &str, gitignore_patterns: &[String]) -> bool {
    for pat in gitignore_patterns {
        let pat_clean = pat.trim_end_matches('/');
        if glob_match(pat_clean, name) {
            return true;
        }
    }
    false
}

fn rg_collect_dir(
    dir: &std::path::Path,
    ctx: &CommandContext,
    type_includes: &[&str],
    type_excludes: &[&str],
    globs: &[&str],
    gitignore_patterns: &[String],
    result: &mut Vec<(String, String)>,
) {
    let entries = match ctx.fs.readdir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut sorted = entries;
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    for entry in &sorted {
        // Skip hidden files/directories
        if entry.name.starts_with('.') {
            continue;
        }

        if rg_is_gitignored(&entry.name, gitignore_patterns) {
            continue;
        }

        let child = dir.join(&entry.name);

        match entry.node_type {
            crate::vfs::NodeType::Directory => {
                rg_collect_dir(
                    &child,
                    ctx,
                    type_includes,
                    type_excludes,
                    globs,
                    gitignore_patterns,
                    result,
                );
            }
            crate::vfs::NodeType::File => {
                if !rg_file_matches_type(&entry.name, type_includes) {
                    continue;
                }
                if rg_file_excluded_type(&entry.name, type_excludes) {
                    continue;
                }
                // Check glob filters
                if !globs.is_empty() {
                    let matches_any = globs.iter().any(|g| {
                        if let Some(neg) = g.strip_prefix('!') {
                            !glob_match(neg, &entry.name)
                        } else {
                            glob_match(g, &entry.name)
                        }
                    });
                    if !matches_any {
                        continue;
                    }
                }

                let display = child.to_string_lossy().to_string();
                if let Ok(bytes) = ctx.fs.read_file(&child) {
                    // Skip binary files
                    let sample = &bytes[..bytes.len().min(512)];
                    if sample.contains(&0) {
                        continue;
                    }
                    result.push((display, String::from_utf8_lossy(&bytes).to_string()));
                }
            }
            _ => {}
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
        fs.write_file(Path::new("/lines.txt"), b"banana\napple\ncherry\napple\n")
            .unwrap();
        fs.write_file(Path::new("/nums.txt"), b"3\n1\n2\n10\n")
            .unwrap();
        fs.write_file(Path::new("/data.txt"), b"a:b:c\nd:e:f\n")
            .unwrap();
        fs.write_file(Path::new("/empty.txt"), b"").unwrap();
        (
            fs,
            HashMap::new(),
            ExecutionLimits::default(),
            NetworkPolicy::default(),
        )
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

    fn ctx<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        network_policy: &'a NetworkPolicy,
    ) -> CommandContext<'a> {
        ctx_with_stdin(fs, env, limits, network_policy, "")
    }

    // ── grep tests ───────────────────────────────────────────────────

    #[test]
    fn grep_basic_match() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "apple\napple\n");
    }

    #[test]
    fn grep_no_match() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["grape".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn grep_case_insensitive() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-i".into(), "APPLE".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "apple\napple\n");
    }

    #[test]
    fn grep_invert() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-v".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "banana\ncherry\n");
    }

    #[test]
    fn grep_line_numbers() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-n".into(), "apple".into(), "lines.txt".into()], &c);
        assert!(r.stdout.contains("2:apple"));
        assert!(r.stdout.contains("4:apple"));
    }

    #[test]
    fn grep_count() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-c".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "2\n");
    }

    #[test]
    fn grep_files_with_matches() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-l".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "lines.txt\n");
    }

    #[test]
    fn grep_stdin() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello\nworld\nhello\n");
        let r = GrepCommand.execute(&["hello".into()], &c);
        assert_eq!(r.stdout, "hello\nhello\n");
    }

    #[test]
    fn grep_missing_pattern() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 2);
    }

    #[test]
    fn grep_fixed_string() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a.b\na*b\n");
        let r = GrepCommand.execute(&["-F".into(), "a.b".into()], &c);
        assert_eq!(r.stdout, "a.b\n");
    }

    #[test]
    fn grep_extended_regexp_alternation() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "cat\ndog\nbird\n");
        let r = GrepCommand.execute(&["-E".into(), "cat|dog".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "cat\ndog\n");
    }

    #[test]
    fn grep_extended_regexp_groups() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "abcabc\nabc\nab\n");
        let r = GrepCommand.execute(&["-E".into(), "(abc)+".into()], &c);
        assert_eq!(r.stdout, "abcabc\nabc\n");
    }

    #[test]
    fn grep_basic_regexp_bre_translation() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "abc\ndef\n");
        // In BRE, \(abc\) should become group (abc) in ERE
        let r = GrepCommand.execute(&["-G".into(), r"\(abc\)".to_string()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "abc\n");
    }

    #[test]
    fn grep_perl_regexp_warns() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello\nworld\n");
        let r = GrepCommand.execute(&["-P".into(), "hello".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "hello\n");
        assert!(r.stderr.contains("warning: -P is not fully supported"));
    }

    #[test]
    fn grep_word_regexp() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "cat concatenate\nthe cat sat\n");
        let r = GrepCommand.execute(&["-w".into(), "cat".into()], &c);
        assert_eq!(r.exit_code, 0);
        // Both lines contain "cat" as a whole word
        assert_eq!(r.stdout, "cat concatenate\nthe cat sat\n");
    }

    #[test]
    fn grep_word_regexp_no_partial() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "concatenate\n");
        let r = GrepCommand.execute(&["-ow".into(), "cat".into()], &c);
        // "cat" does not appear as whole word in "concatenate"
        assert_eq!(r.exit_code, 1);
        assert_eq!(r.stdout, "");
    }

    #[test]
    fn grep_line_regexp() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello\nhello world\n");
        let r = GrepCommand.execute(&["-x".into(), "hello".into()], &c);
        assert_eq!(r.stdout, "hello\n");
    }

    #[test]
    fn grep_recursive_search() {
        let (fs, env, limits, _np) = setup();
        fs.mkdir_p(Path::new("/project/src")).unwrap();
        fs.write_file(Path::new("/project/src/main.rs"), b"fn main() {}\n")
            .unwrap();
        fs.write_file(Path::new("/project/src/lib.rs"), b"pub fn hello() {}\n")
            .unwrap();
        fs.write_file(Path::new("/project/README.md"), b"hello world\n")
            .unwrap();

        let c = CommandContext {
            fs: &*fs,
            cwd: "/project",
            env: &env,
            stdin: "",
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let r = GrepCommand.execute(&["-r".into(), "hello".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("hello"));
        // Should show filenames in recursive mode
        assert!(r.stdout.contains(":"));
    }

    #[test]
    fn grep_recursive_with_include() {
        let (fs, env, limits, _np) = setup();
        fs.mkdir_p(Path::new("/proj/src")).unwrap();
        fs.write_file(Path::new("/proj/src/main.rs"), b"fn hello() {}\n")
            .unwrap();
        fs.write_file(Path::new("/proj/src/readme.txt"), b"hello docs\n")
            .unwrap();

        let c = CommandContext {
            fs: &*fs,
            cwd: "/proj",
            env: &env,
            stdin: "",
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let r = GrepCommand.execute(&["-r".into(), "--include=*.txt".into(), "hello".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("readme.txt"));
        assert!(!r.stdout.contains("main.rs"));
    }

    #[test]
    fn grep_recursive_with_exclude() {
        let (fs, env, limits, _np) = setup();
        fs.mkdir_p(Path::new("/proj2/logs")).unwrap();
        fs.write_file(Path::new("/proj2/data.txt"), b"error found\n")
            .unwrap();
        fs.write_file(Path::new("/proj2/logs/app.log"), b"error occurred\n")
            .unwrap();

        let c = CommandContext {
            fs: &*fs,
            cwd: "/proj2",
            env: &env,
            stdin: "",
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let r = GrepCommand.execute(&["-r".into(), "--exclude=*.log".into(), "error".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("data.txt"));
        assert!(!r.stdout.contains("app.log"));
    }

    #[test]
    fn grep_after_context() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(
            &*fs,
            &env,
            &limits,
            &np,
            "line1\nline2\nmatch\nline4\nline5\nline6\n",
        );
        let r = GrepCommand.execute(&["-A".into(), "2".into(), "match".into()], &c);
        assert!(r.stdout.contains("match\n"));
        assert!(r.stdout.contains("line4"));
        assert!(r.stdout.contains("line5"));
        assert!(!r.stdout.contains("line6"));
    }

    #[test]
    fn grep_before_context() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(
            &*fs,
            &env,
            &limits,
            &np,
            "line1\nline2\nmatch\nline4\nline5\n",
        );
        let r = GrepCommand.execute(&["-B".into(), "2".into(), "match".into()], &c);
        assert!(r.stdout.contains("line1"));
        assert!(r.stdout.contains("line2"));
        assert!(r.stdout.contains("match"));
        assert!(!r.stdout.contains("line4"));
    }

    #[test]
    fn grep_context_both() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\nb\nc\nmatch\ne\nf\ng\n");
        let r = GrepCommand.execute(&["-C".into(), "1".into(), "match".into()], &c);
        assert!(r.stdout.contains("c\n"));
        assert!(r.stdout.contains("match\n"));
        assert!(r.stdout.contains("e\n"));
        assert!(!r.stdout.contains("a\n"));
    }

    #[test]
    fn grep_context_separator() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\nmatch1\nb\nc\nd\nmatch2\ne\n");
        let r = GrepCommand.execute(&["-C".into(), "0".into(), "match".into()], &c);
        assert!(r.stdout.contains("match1\n--\nmatch2\n"));
    }

    #[test]
    fn grep_only_matching() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "foo123bar\nhello456\n");
        let r = GrepCommand.execute(&["-oE".into(), "[0-9]+".into()], &c);
        assert_eq!(r.stdout, "123\n456\n");
    }

    #[test]
    fn grep_with_filename() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-H".into(), "apple".into(), "lines.txt".into()], &c);
        assert!(r.stdout.contains("lines.txt:apple"));
    }

    #[test]
    fn grep_no_filename() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(
            &[
                "-h".into(),
                "apple".into(),
                "lines.txt".into(),
                "lines.txt".into(),
            ],
            &c,
        );
        // With -h, no filename even with multiple files
        assert!(!r.stdout.contains("lines.txt:"));
        assert_eq!(r.stdout, "apple\napple\napple\napple\n");
    }

    #[test]
    fn grep_quiet_match() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-q".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "");
    }

    #[test]
    fn grep_quiet_no_match() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-q".into(), "grape".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert_eq!(r.stdout, "");
    }

    #[test]
    fn grep_max_count() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(
            &["-m".into(), "1".into(), "apple".into(), "lines.txt".into()],
            &c,
        );
        assert_eq!(r.stdout, "apple\n");
    }

    #[test]
    fn grep_multiple_patterns_with_e() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(
            &[
                "-e".into(),
                "apple".into(),
                "-e".into(),
                "cherry".into(),
                "lines.txt".into(),
            ],
            &c,
        );
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("apple"));
        assert!(r.stdout.contains("cherry"));
        assert!(!r.stdout.contains("banana"));
    }

    #[test]
    fn grep_patterns_from_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/patterns.txt"), b"apple\ncherry\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(
            &["-f".into(), "patterns.txt".into(), "lines.txt".into()],
            &c,
        );
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("apple"));
        assert!(r.stdout.contains("cherry"));
        assert!(!r.stdout.contains("banana"));
    }

    #[test]
    fn grep_files_without_match() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(
            &[
                "-L".into(),
                "grape".into(),
                "lines.txt".into(),
                "nums.txt".into(),
            ],
            &c,
        );
        assert!(r.stdout.contains("lines.txt"));
        assert!(r.stdout.contains("nums.txt"));
    }

    #[test]
    fn grep_files_without_match_partial() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(
            &[
                "-L".into(),
                "apple".into(),
                "lines.txt".into(),
                "nums.txt".into(),
            ],
            &c,
        );
        // lines.txt has "apple" so it should NOT be listed
        assert!(!r.stdout.contains("lines.txt"));
        // nums.txt has no "apple" so it SHOULD be listed
        assert!(r.stdout.contains("nums.txt"));
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn grep_files_without_match_all_matched() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        // "a" appears in both lines.txt and data.txt
        let r = GrepCommand.execute(
            &[
                "-L".into(),
                "a".into(),
                "lines.txt".into(),
                "data.txt".into(),
            ],
            &c,
        );
        // Both files have "a", so no files printed → exit 1
        assert_eq!(r.stdout, "");
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn grep_combined_short_flags() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-in".into(), "apple".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("2:apple"));
        assert!(r.stdout.contains("4:apple"));
    }

    #[test]
    fn grep_combined_recursive_insensitive_line_numbers() {
        let (fs, env, limits, _np) = setup();
        fs.mkdir_p(Path::new("/rtest")).unwrap();
        fs.write_file(Path::new("/rtest/a.txt"), b"Hello\nworld\n")
            .unwrap();

        let c = CommandContext {
            fs: &*fs,
            cwd: "/rtest",
            env: &env,
            stdin: "",
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let r = GrepCommand.execute(&["-rin".into(), "hello".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("1:") || r.stdout.contains(":1:"));
        assert!(r.stdout.contains("Hello"));
    }

    #[test]
    fn grep_empty_pattern_matches_all() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "line1\nline2\n");
        let r = GrepCommand.execute(&["".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "line1\nline2\n");
    }

    #[test]
    fn grep_no_matches_exit_code_1() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "aaa\nbbb\n");
        let r = GrepCommand.execute(&["zzz".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert_eq!(r.stdout, "");
    }

    #[test]
    fn grep_long_flags() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(
            &[
                "--ignore-case".into(),
                "--line-number".into(),
                "APPLE".into(),
                "lines.txt".into(),
            ],
            &c,
        );
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("2:apple"));
    }

    #[test]
    fn grep_e_combined_value() {
        // Test -e with value attached: -epattern
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-eapple".into(), "lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "apple\napple\n");
    }

    #[test]
    fn grep_context_with_line_numbers() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\nb\nMATCH\nd\ne\n");
        let r = GrepCommand.execute(
            &[
                "-n".into(),
                "-B".into(),
                "1".into(),
                "-A".into(),
                "1".into(),
                "MATCH".into(),
            ],
            &c,
        );
        assert!(r.stdout.contains("2-b"));
        assert!(r.stdout.contains("3:MATCH"));
        assert!(r.stdout.contains("4-d"));
    }

    #[test]
    fn grep_max_count_with_context() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\nmatch\nb\nmatch\nc\n");
        let r = GrepCommand.execute(
            &[
                "-m".into(),
                "1".into(),
                "-A".into(),
                "1".into(),
                "match".into(),
            ],
            &c,
        );
        // Only first match + context
        assert!(r.stdout.contains("match\n"));
        assert!(r.stdout.contains("b\n"));
        // Should not have second match
        let match_count = r.stdout.matches("match").count();
        assert_eq!(match_count, 1);
    }

    #[test]
    fn grep_recursive_on_explicit_directory() {
        let (fs, env, limits, np) = setup();
        fs.mkdir_p(Path::new("/searchdir/sub")).unwrap();
        fs.write_file(Path::new("/searchdir/a.txt"), b"found it\n")
            .unwrap();
        fs.write_file(Path::new("/searchdir/sub/b.txt"), b"found it too\n")
            .unwrap();

        let c = ctx(&*fs, &env, &limits, &np);
        let r = GrepCommand.execute(&["-r".into(), "found".into(), "/searchdir".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("found it"));
        assert!(r.stdout.contains("found it too"));
    }

    #[test]
    fn grep_only_matching_with_filename() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "abc123def\n");
        let r = GrepCommand.execute(&["-oHE".into(), "[0-9]+".into()], &c);
        assert!(r.stdout.contains("(standard input):123"));
    }

    #[test]
    fn grep_context_long_flag_equals() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\nb\nMATCH\nd\ne\n");
        let r = GrepCommand.execute(&["--context=1".into(), "MATCH".into()], &c);
        assert!(r.stdout.contains("b\n"));
        assert!(r.stdout.contains("MATCH\n"));
        assert!(r.stdout.contains("d\n"));
    }

    #[test]
    fn grep_bre_default_plus_literal() {
        // Default mode is BRE: bare + is literal
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a+b\naab\n");
        let r = GrepCommand.execute(&["a+b".into()], &c);
        assert_eq!(r.stdout, "a+b\n");
    }

    #[test]
    fn grep_bre_escaped_plus_is_quantifier() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "aab\nab\nb\n");
        let r = GrepCommand.execute(&[r"a\+b".to_string()], &c);
        // \+ in BRE means one-or-more: matches "aab" and "ab"
        assert!(r.stdout.contains("aab"));
        assert!(r.stdout.contains("ab"));
        assert!(!r.stdout.contains("\nb\n"));
    }

    #[test]
    fn grep_bre_pipe_literal() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a|b\nab\n");
        let r = GrepCommand.execute(&["a|b".into()], &c);
        // Bare | is literal in BRE
        assert_eq!(r.stdout, "a|b\n");
    }

    #[test]
    fn grep_end_of_options_separator() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "-v\nhello\n");
        // Search for literal "-v" using -- separator
        let r = GrepCommand.execute(&["--".into(), "-v".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "-v\n");
    }

    #[test]
    fn grep_e_pattern_starting_with_dash() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "-v\nhello\n");
        let r = GrepCommand.execute(&["-e".into(), "-v".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "-v\n");
    }

    #[test]
    fn grep_recursive_no_matches_exit_1() {
        let (fs, env, limits, _np) = setup();
        fs.mkdir_p(Path::new("/nomatch")).unwrap();
        fs.write_file(Path::new("/nomatch/a.txt"), b"hello\n")
            .unwrap();
        let c = CommandContext {
            fs: &*fs,
            cwd: "/nomatch",
            env: &env,
            stdin: "",
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let r = GrepCommand.execute(&["-r".into(), "zzzzz".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn grep_word_and_line_regexp_combined() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "cat\ncat dog\n");
        let r = GrepCommand.execute(&["-xw".into(), "cat".into()], &c);
        assert_eq!(r.stdout, "cat\n");
    }

    // ── sort tests ───────────────────────────────────────────────────

    #[test]
    fn sort_basic() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SortCommand.execute(&["lines.txt".into()], &c);
        assert_eq!(r.stdout, "apple\napple\nbanana\ncherry\n");
    }

    #[test]
    fn sort_reverse() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SortCommand.execute(&["-r".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "cherry\nbanana\napple\napple\n");
    }

    #[test]
    fn sort_numeric() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SortCommand.execute(&["-n".into(), "nums.txt".into()], &c);
        assert_eq!(r.stdout, "1\n2\n3\n10\n");
    }

    #[test]
    fn sort_numeric_with_leading_spaces() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(
            &*fs,
            &env,
            &limits,
            &np,
            "      3 eng\n      1 dept\n      2 sales\n",
        );
        let r = SortCommand.execute(&["-rn".into()], &c);
        assert_eq!(r.stdout, "      3 eng\n      2 sales\n      1 dept\n");
    }

    #[test]
    fn parse_leading_number_cases() {
        assert_eq!(parse_leading_number("  3 eng"), 3.0);
        assert_eq!(parse_leading_number("-5.3 foo"), -5.3);
        assert_eq!(parse_leading_number("abc"), 0.0);
        assert_eq!(parse_leading_number(""), 0.0);
        assert_eq!(parse_leading_number("-"), 0.0);
        assert_eq!(parse_leading_number(".5"), 0.5);
        assert_eq!(parse_leading_number("  +42rest"), 42.0);
    }

    #[test]
    fn sort_unique() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = SortCommand.execute(&["-u".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "apple\nbanana\ncherry\n");
    }

    #[test]
    fn sort_stdin() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "z\na\nm\n");
        let r = SortCommand.execute(&[], &c);
        assert_eq!(r.stdout, "a\nm\nz\n");
    }

    // ── uniq tests ───────────────────────────────────────────────────

    #[test]
    fn uniq_basic() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "aaa\naaa\nbbb\nccc\nccc\n");
        let r = UniqCommand.execute(&[], &c);
        assert_eq!(r.stdout, "aaa\nbbb\nccc\n");
    }

    #[test]
    fn uniq_count() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\na\nb\n");
        let r = UniqCommand.execute(&["-c".into()], &c);
        assert!(r.stdout.contains("2 a"));
        assert!(r.stdout.contains("1 b"));
    }

    #[test]
    fn uniq_duplicates_only() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\na\nb\nc\nc\n");
        let r = UniqCommand.execute(&["-d".into()], &c);
        assert_eq!(r.stdout, "a\nc\n");
    }

    #[test]
    fn uniq_unique_only() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\na\nb\nc\nc\n");
        let r = UniqCommand.execute(&["-u".into()], &c);
        assert_eq!(r.stdout, "b\n");
    }

    // ── cut tests ────────────────────────────────────────────────────

    #[test]
    fn cut_fields() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
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
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello\nworld\n");
        let r = CutCommand.execute(&["-c".into(), "1-3".into()], &c);
        assert_eq!(r.stdout, "hel\nwor\n");
    }

    #[test]
    fn cut_missing_spec() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CutCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── head tests ───────────────────────────────────────────────────

    #[test]
    fn head_default() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(
            &*fs,
            &env,
            &limits,
            &np,
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
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = HeadCommand.execute(&["-n".into(), "2".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "banana\napple\n");
    }

    #[test]
    fn head_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = HeadCommand.execute(&["-n".into(), "1".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "banana\n");
    }

    // ── tail tests ───────────────────────────────────────────────────

    #[test]
    fn tail_default() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(
            &*fs,
            &env,
            &limits,
            &np,
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
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TailCommand.execute(&["-n".into(), "2".into(), "lines.txt".into()], &c);
        assert_eq!(r.stdout, "cherry\napple\n");
    }

    // ── wc tests ─────────────────────────────────────────────────────

    #[test]
    fn wc_all() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello world\nfoo\n");
        let r = WcCommand.execute(&[], &c);
        assert!(r.stdout.contains("2")); // lines
        assert!(r.stdout.contains("3")); // words
    }

    #[test]
    fn wc_lines_only() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\nb\nc\n");
        let r = WcCommand.execute(&["-l".into()], &c);
        assert!(r.stdout.contains("3"));
    }

    #[test]
    fn wc_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = WcCommand.execute(&["-l".into(), "lines.txt".into()], &c);
        assert!(r.stdout.contains("4"));
    }

    // ── tr tests ─────────────────────────────────────────────────────

    #[test]
    fn tr_translate() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello");
        let r = TrCommand.execute(&["a-z".into(), "A-Z".into()], &c);
        assert_eq!(r.stdout, "HELLO");
    }

    #[test]
    fn tr_delete() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello world");
        let r = TrCommand.execute(&["-d".into(), " ".into()], &c);
        assert_eq!(r.stdout, "helloworld");
    }

    #[test]
    fn tr_squeeze() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "aabbcc");
        let r = TrCommand.execute(&["-s".into(), "a-z".into()], &c);
        assert_eq!(r.stdout, "abc");
    }

    #[test]
    fn tr_missing_operand() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TrCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── rev tests ────────────────────────────────────────────────────

    #[test]
    fn rev_basic() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello\nworld\n");
        let r = RevCommand.execute(&[], &c);
        assert_eq!(r.stdout, "olleh\ndlrow\n");
    }

    // ── fold tests ───────────────────────────────────────────────────

    #[test]
    fn fold_default_width() {
        let (fs, env, limits, np) = setup();
        let short = "short\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, short);
        let r = FoldCommand.execute(&[], &c);
        assert_eq!(r.stdout, "short\n");
    }

    #[test]
    fn fold_custom_width() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "abcdefghij\n");
        let r = FoldCommand.execute(&["-w".into(), "5".into()], &c);
        assert_eq!(r.stdout, "abcde\nfghij\n");
    }

    // ── nl tests ─────────────────────────────────────────────────────

    #[test]
    fn nl_basic() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "first\nsecond\n");
        let r = NlCommand.execute(&[], &c);
        assert!(r.stdout.contains("1\tfirst"));
        assert!(r.stdout.contains("2\tsecond"));
    }

    #[test]
    fn nl_empty_line_not_numbered() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\n\nb\n");
        let r = NlCommand.execute(&[], &c);
        assert!(r.stdout.contains("1\ta"));
        assert!(r.stdout.contains("2\tb"));
    }

    // ── printf tests ─────────────────────────────────────────────────

    #[test]
    fn printf_string() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["hello %s\n".into(), "world".into()], &c);
        assert_eq!(r.stdout, "hello world\n");
    }

    #[test]
    fn printf_int() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%d\n".into(), "42".into()], &c);
        assert_eq!(r.stdout, "42\n");
    }

    #[test]
    fn printf_hex() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%x\n".into(), "255".into()], &c);
        assert_eq!(r.stdout, "ff\n");
    }

    #[test]
    fn printf_octal() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%o\n".into(), "8".into()], &c);
        assert_eq!(r.stdout, "10\n");
    }

    #[test]
    fn printf_percent() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["100%%\n".into()], &c);
        assert_eq!(r.stdout, "100%\n");
    }

    #[test]
    fn printf_no_args() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn printf_multiple_args_cycle() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%s\n".into(), "a".into(), "b".into(), "c".into()], &c);
        assert_eq!(r.stdout, "a\nb\nc\n");
    }

    #[test]
    fn printf_zero_padded_int() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%05d".into(), "42".into()], &c);
        assert_eq!(r.stdout, "00042");
    }

    #[test]
    fn printf_left_aligned_string() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%-10s|".into(), "hi".into()], &c);
        assert_eq!(r.stdout, "hi        |");
    }

    #[test]
    fn printf_right_aligned_string() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%10s|".into(), "hi".into()], &c);
        assert_eq!(r.stdout, "        hi|");
    }

    #[test]
    fn printf_precision_float() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%.2f".into(), "3.14159".into()], &c);
        assert_eq!(r.stdout, "3.14");
    }

    #[test]
    fn printf_width_and_precision_float() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%10.2f".into(), "3.14".into()], &c);
        assert_eq!(r.stdout, "      3.14");
    }

    #[test]
    fn printf_plus_sign_int() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%+d".into(), "42".into()], &c);
        assert_eq!(r.stdout, "+42");
    }

    #[test]
    fn printf_plus_sign_negative_int() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%+d".into(), "-5".into()], &c);
        assert_eq!(r.stdout, "-5");
    }

    #[test]
    fn printf_alt_form_hex() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%#x".into(), "255".into()], &c);
        assert_eq!(r.stdout, "0xff");
    }

    #[test]
    fn printf_alt_form_octal() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%#o".into(), "8".into()], &c);
        assert_eq!(r.stdout, "010");
    }

    #[test]
    fn printf_string_precision_truncates() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%.3s".into(), "hello".into()], &c);
        assert_eq!(r.stdout, "hel");
    }

    #[test]
    fn printf_star_width() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PrintfCommand.execute(&["%*d".into(), "8".into(), "42".into()], &c);
        assert_eq!(r.stdout, "      42");
    }

    // ── paste tests ──────────────────────────────────────────────────

    #[test]
    fn paste_basic() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/p1.txt"), b"a\nb\n").unwrap();
        fs.write_file(Path::new("/p2.txt"), b"1\n2\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PasteCommand.execute(&["p1.txt".into(), "p2.txt".into()], &c);
        assert_eq!(r.stdout, "a\t1\nb\t2\n");
    }

    #[test]
    fn paste_custom_delimiter() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/p1.txt"), b"a\nb\n").unwrap();
        fs.write_file(Path::new("/p2.txt"), b"1\n2\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = PasteCommand.execute(
            &["-d".into(), ",".into(), "p1.txt".into(), "p2.txt".into()],
            &c,
        );
        assert_eq!(r.stdout, "a,1\nb,2\n");
    }

    #[test]
    fn paste_stdin() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "x\ny\n");
        let r = PasteCommand.execute(&[], &c);
        assert_eq!(r.stdout, "x\ny\n");
    }

    // ── tac tests ────────────────────────────────────────────────────

    #[test]
    fn tac_reverse_lines() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a\nb\nc\n");
        let r = TacCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "c\nb\na\n");
    }

    #[test]
    fn tac_from_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TacCommand.execute(&["lines.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "apple\ncherry\napple\nbanana\n");
    }

    #[test]
    fn tac_single_line() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "only\n");
        let r = TacCommand.execute(&[], &c);
        assert_eq!(r.stdout, "only\n");
    }

    #[test]
    fn tac_empty_input() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "");
        let r = TacCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "");
    }

    #[test]
    fn tac_custom_separator() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "a:b:c");
        let r = TacCommand.execute(&["-s".into(), ":".into()], &c);
        assert_eq!(r.stdout, "c:b:a\n");
    }

    // ── comm tests ───────────────────────────────────────────────────

    #[test]
    fn comm_basic() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/sorted1.txt"), b"a\nb\nd\n")
            .unwrap();
        fs.write_file(Path::new("/sorted2.txt"), b"b\nc\nd\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CommCommand.execute(&["sorted1.txt".into(), "sorted2.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "a\n\t\tb\n\tc\n\t\td\n");
    }

    #[test]
    fn comm_suppress_col1() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/s1.txt"), b"a\nb\nd\n").unwrap();
        fs.write_file(Path::new("/s2.txt"), b"b\nc\nd\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CommCommand.execute(&["-1".into(), "s1.txt".into(), "s2.txt".into()], &c);
        assert_eq!(r.stdout, "\tb\nc\n\td\n");
    }

    #[test]
    fn comm_suppress_col2() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/s1.txt"), b"a\nb\nd\n").unwrap();
        fs.write_file(Path::new("/s2.txt"), b"b\nc\nd\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CommCommand.execute(&["-2".into(), "s1.txt".into(), "s2.txt".into()], &c);
        assert_eq!(r.stdout, "a\n\tb\n\td\n");
    }

    #[test]
    fn comm_suppress_col3() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/s1.txt"), b"a\nb\nd\n").unwrap();
        fs.write_file(Path::new("/s2.txt"), b"b\nc\nd\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CommCommand.execute(&["-3".into(), "s1.txt".into(), "s2.txt".into()], &c);
        assert_eq!(r.stdout, "a\n\tc\n");
    }

    #[test]
    fn comm_suppress_col12() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/s1.txt"), b"a\nb\nd\n").unwrap();
        fs.write_file(Path::new("/s2.txt"), b"b\nc\nd\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CommCommand.execute(&["-12".into(), "s1.txt".into(), "s2.txt".into()], &c);
        assert_eq!(r.stdout, "b\nd\n");
    }

    // ── join tests ───────────────────────────────────────────────────

    #[test]
    fn join_basic() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/j1.txt"), b"1 Alice\n2 Bob\n3 Carol\n")
            .unwrap();
        fs.write_file(Path::new("/j2.txt"), b"1 NY\n2 LA\n4 SF\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = JoinCommand.execute(&["j1.txt".into(), "j2.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "1 Alice NY\n2 Bob LA\n");
    }

    #[test]
    fn join_with_separator() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/j1.txt"), b"1:Alice\n2:Bob\n")
            .unwrap();
        fs.write_file(Path::new("/j2.txt"), b"1:NY\n2:LA\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = JoinCommand.execute(
            &["-t".into(), ":".into(), "j1.txt".into(), "j2.txt".into()],
            &c,
        );
        assert_eq!(r.stdout, "1:Alice:NY\n2:Bob:LA\n");
    }

    #[test]
    fn join_unpairable() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/j1.txt"), b"1 Alice\n2 Bob\n3 Carol\n")
            .unwrap();
        fs.write_file(Path::new("/j2.txt"), b"1 NY\n2 LA\n4 SF\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = JoinCommand.execute(
            &["-a".into(), "1".into(), "j1.txt".into(), "j2.txt".into()],
            &c,
        );
        assert!(r.stdout.contains("1 Alice NY"));
        assert!(r.stdout.contains("2 Bob LA"));
        assert!(r.stdout.contains("3 Carol"));
    }

    // ── fmt tests ────────────────────────────────────────────────────

    #[test]
    fn fmt_reflow_paragraph() {
        let (fs, env, limits, np) = setup();
        let input =
            "This is a long line that should be reflowed to fit within forty characters width.\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, input);
        let r = FmtCommand.execute(&["-w".into(), "40".into()], &c);
        assert_eq!(r.exit_code, 0);
        for line in r.stdout.lines() {
            assert!(
                line.len() <= 40,
                "Line too long: {:?} ({})",
                line,
                line.len()
            );
        }
        // All words must be preserved
        let original_words: Vec<&str> = input.split_whitespace().collect();
        let output_words: Vec<&str> = r.stdout.split_whitespace().collect();
        assert_eq!(original_words, output_words);
    }

    #[test]
    fn fmt_preserves_paragraph_breaks() {
        let (fs, env, limits, np) = setup();
        let input = "Para one.\n\nPara two.\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, input);
        let r = FmtCommand.execute(&["-w".into(), "75".into()], &c);
        assert!(
            r.stdout.contains("\n\n"),
            "Should preserve blank line between paragraphs"
        );
    }

    #[test]
    fn fmt_split_only() {
        let (fs, env, limits, np) = setup();
        let input = "short\nvery long line that exceeds twenty characters in width\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, input);
        let r = FmtCommand.execute(&["-s".into(), "-w".into(), "20".into()], &c);
        // Short lines should NOT be joined
        assert!(r.stdout.starts_with("short\n"));
    }

    #[test]
    fn fmt_empty_input() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "");
        let r = FmtCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "");
    }

    // ── column tests ─────────────────────────────────────────────────

    #[test]
    fn column_table_mode() {
        let (fs, env, limits, np) = setup();
        let input = "name age city\nAlice 30 NYC\nBob 25 LA\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, input);
        let r = ColumnCommand.execute(&["-t".into()], &c);
        assert_eq!(r.exit_code, 0);
        let lines: Vec<&str> = r.stdout.lines().collect();
        assert_eq!(lines.len(), 3);
        // Columns should be aligned
        assert!(lines[0].contains("name"));
        assert!(lines[0].contains("age"));
        assert!(lines[0].contains("city"));
    }

    #[test]
    fn column_table_custom_sep() {
        let (fs, env, limits, np) = setup();
        let input = "Alice:30:NYC\nBob:25:LA\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, input);
        let r = ColumnCommand.execute(&["-t".into(), "-s".into(), ":".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("Alice"));
        assert!(r.stdout.contains("NYC"));
    }

    #[test]
    fn column_table_custom_output_sep() {
        let (fs, env, limits, np) = setup();
        let input = "a 1\nb 2\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, input);
        let r = ColumnCommand.execute(&["-t".into(), "-o".into(), " | ".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains(" | "));
    }

    #[test]
    fn column_empty_input() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "");
        let r = ColumnCommand.execute(&["-t".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "");
    }

    // ── expand tests ─────────────────────────────────────────────────

    #[test]
    fn expand_default() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "\thello\n");
        let r = ExpandCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "        hello\n");
    }

    #[test]
    fn expand_custom_width() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "\thello\n");
        let r = ExpandCommand.execute(&["-t".into(), "4".into()], &c);
        assert_eq!(r.stdout, "    hello\n");
    }

    #[test]
    fn expand_tab_positions() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "\ta\tb\n");
        let r = ExpandCommand.execute(&["-t".into(), "4,8".into()], &c);
        assert_eq!(r.stdout, "    a   b\n");
    }

    #[test]
    fn expand_mid_line_tab() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "ab\tcd\n");
        let r = ExpandCommand.execute(&["-t".into(), "8".into()], &c);
        assert_eq!(r.stdout, "ab      cd\n");
    }

    #[test]
    fn expand_from_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/tabs.txt"), b"\thello\n\tworld\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ExpandCommand.execute(&["-t".into(), "4".into(), "tabs.txt".into()], &c);
        assert_eq!(r.stdout, "    hello\n    world\n");
    }

    #[test]
    fn expand_empty_input() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "");
        let r = ExpandCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "");
    }

    // ── unexpand tests ───────────────────────────────────────────────

    #[test]
    fn unexpand_leading_spaces() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "        hello\n");
        let r = UnexpandCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "\thello\n");
    }

    #[test]
    fn unexpand_custom_width() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "    hello\n");
        let r = UnexpandCommand.execute(&["-t".into(), "4".into()], &c);
        assert_eq!(r.stdout, "\thello\n");
    }

    #[test]
    fn unexpand_all_spaces() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello   world\n");
        let r = UnexpandCommand.execute(&["-a".into(), "-t".into(), "4".into()], &c);
        // "hello   world" - 'hello' takes cols 0-4, then 3 spaces at cols 5,6,7 -> tab at 8
        assert_eq!(r.stdout, "hello\tworld\n");
    }

    #[test]
    fn unexpand_no_convert_middle_without_a() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "hello        world\n");
        let r = UnexpandCommand.execute(&[], &c);
        // Without -a, only leading spaces are converted; there are no leading spaces
        assert_eq!(r.stdout, "hello        world\n");
    }

    #[test]
    fn unexpand_empty_input() {
        let (fs, env, limits, np) = setup();
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, "");
        let r = UnexpandCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "");
    }

    // ── additional edge-case tests ───────────────────────────────────

    #[test]
    fn join_a2_unpairable_file2() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/j1.txt"), b"1 Alice\n2 Bob\n")
            .unwrap();
        fs.write_file(Path::new("/j2.txt"), b"1 NY\n3 SF\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = JoinCommand.execute(
            &["-a".into(), "2".into(), "j1.txt".into(), "j2.txt".into()],
            &c,
        );
        assert_eq!(r.stdout, "1 Alice NY\n3 SF\n");
    }

    #[test]
    fn join_duplicate_keys_cross_product() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/j1.txt"), b"1 A\n1 B\n").unwrap();
        fs.write_file(Path::new("/j2.txt"), b"1 X\n1 Y\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = JoinCommand.execute(&["j1.txt".into(), "j2.txt".into()], &c);
        assert_eq!(r.stdout, "1 A X\n1 A Y\n1 B X\n1 B Y\n");
    }

    #[test]
    fn join_output_format() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/j1.txt"), b"1 Alice\n2 Bob\n")
            .unwrap();
        fs.write_file(Path::new("/j2.txt"), b"1 NY\n2 LA\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = JoinCommand.execute(
            &[
                "-o".into(),
                "0,2.2,1.2".into(),
                "j1.txt".into(),
                "j2.txt".into(),
            ],
            &c,
        );
        assert_eq!(r.stdout, "1 NY Alice\n2 LA Bob\n");
    }

    #[test]
    fn join_empty_replacement() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/j1.txt"), b"1 Alice\n2 Bob\n")
            .unwrap();
        fs.write_file(Path::new("/j2.txt"), b"1 NY\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = JoinCommand.execute(
            &[
                "-a".into(),
                "1".into(),
                "-e".into(),
                "EMPTY".into(),
                "-o".into(),
                "0,1.2,2.2".into(),
                "j1.txt".into(),
                "j2.txt".into(),
            ],
            &c,
        );
        assert_eq!(r.stdout, "1 Alice NY\n2 Bob EMPTY\n");
    }

    #[test]
    fn tac_multiple_files() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/t1.txt"), b"a\nb\n").unwrap();
        fs.write_file(Path::new("/t2.txt"), b"c\nd\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TacCommand.execute(&["t1.txt".into(), "t2.txt".into()], &c);
        assert_eq!(r.stdout, "d\nc\nb\na\n");
    }

    #[test]
    fn column_fill_mode() {
        let (fs, env, limits, np) = setup();
        let input = "alpha\nbeta\ngamma\ndelta\n";
        let c = ctx_with_stdin(&*fs, &env, &limits, &np, input);
        let r = ColumnCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 0);
        assert!(!r.stdout.is_empty());
        // All words should appear
        assert!(r.stdout.contains("alpha"));
        assert!(r.stdout.contains("delta"));
    }

    #[test]
    fn comm_with_empty_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/nonempty.txt"), b"a\nb\n")
            .unwrap();
        fs.write_file(Path::new("/mt.txt"), b"").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CommCommand.execute(&["nonempty.txt".into(), "mt.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "a\nb\n");
    }
}
