//! The `sed` stream editor command — a mini-interpreter for sed scripts.

use crate::commands::regex_util::bre_to_ere;
use crate::commands::{CommandContext, CommandMeta, CommandResult};
use regex::Regex;
use std::path::PathBuf;

// ── Data model (4a) ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
enum SedAddress {
    LineNumber(usize),
    Last,
    Regex(Regex),
    Step(usize, usize),
}

#[derive(Debug, Clone)]
enum SedRange {
    Single(SedAddress),
    Range(SedAddress, SedAddress),
}

#[derive(Debug, Clone)]
struct AddressedCmd {
    range: Option<SedRange>,
    negated: bool,
    cmd: SedCmd,
}

#[derive(Debug, Clone)]
struct SubstituteFlags {
    global: bool,
    case_insensitive: bool,
    print: bool,
    nth: Option<usize>,
}

#[derive(Debug, Clone)]
enum SedCmd {
    Substitute {
        regex: Regex,
        replacement: String,
        flags: SubstituteFlags,
    },
    Delete,
    Print,
    Quit,
    Append(String),
    Insert(String),
    Change(String),
    Label(String),
    Branch(Option<String>),
    BranchIfSubstituted(Option<String>),
    BranchIfNotSubstituted(Option<String>),
    HoldGet,
    HoldAppend,
    PatternGet,
    PatternAppend,
    Exchange,
    LineNumber,
    Next,
    NextAppend,
    Transliterate(Vec<char>, Vec<char>),
    CommandGroup(SedScript),
    Noop,
}

type SedScript = Vec<AddressedCmd>;

// ── Options ──────────────────────────────────────────────────────────

struct SedOpts<'a> {
    quiet: bool,
    in_place: bool,
    extended_regex: bool,
    scripts: Vec<&'a str>,
    script_files: Vec<&'a str>,
    files: Vec<&'a str>,
}

// ── Command struct ───────────────────────────────────────────────────

pub struct SedCommand;

static SED_META: CommandMeta = CommandMeta {
    name: "sed",
    synopsis: "sed [-nEi] [-e SCRIPT] [-f FILE] [FILE ...]",
    description: "Stream editor for filtering and transforming text.",
    options: &[
        (
            "-n, --quiet",
            "suppress automatic printing of pattern space",
        ),
        ("-i, --in-place", "edit files in place"),
        ("-E, -r", "use extended regular expressions"),
        ("-e SCRIPT", "add the script to the commands to be executed"),
        ("-f FILE", "add the contents of FILE to the commands"),
    ],
    supports_help_flag: true,
};

impl super::VirtualCommand for SedCommand {
    fn name(&self) -> &str {
        "sed"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&SED_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let opts = match parse_args(args) {
            Ok(o) => o,
            Err(e) => return e,
        };

        // Collect script text
        let mut script_text = String::new();
        for s in &opts.scripts {
            if !script_text.is_empty() {
                script_text.push('\n');
            }
            script_text.push_str(s);
        }
        for sf in &opts.script_files {
            let path = resolve_path(sf, ctx.cwd);
            match ctx.fs.read_file(&path) {
                Ok(bytes) => {
                    if !script_text.is_empty() {
                        script_text.push('\n');
                    }
                    script_text.push_str(&String::from_utf8_lossy(&bytes));
                }
                Err(e) => {
                    return CommandResult {
                        stderr: format!("sed: {}: {}\n", sf, e),
                        exit_code: 2,
                        ..Default::default()
                    };
                }
            }
        }

        // Parse script
        let script = match parse_script(&script_text, opts.extended_regex) {
            Ok(s) => s,
            Err(msg) => {
                return CommandResult {
                    stderr: format!("sed: {}\n", msg),
                    exit_code: 2,
                    ..Default::default()
                };
            }
        };

        // Collect labels for branching
        let labels = collect_labels(&script);

        if opts.in_place {
            if opts.files.is_empty() {
                return CommandResult {
                    stderr: "sed: no input files for in-place editing\n".to_string(),
                    exit_code: 2,
                    ..Default::default()
                };
            }
            let mut stderr = String::new();
            let mut has_errors = false;
            for file in &opts.files {
                let path = resolve_path(file, ctx.cwd);
                let content = match ctx.fs.read_file(&path) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Err(e) => {
                        stderr.push_str(&format!("sed: {}: {}\n", file, e));
                        has_errors = true;
                        continue;
                    }
                };
                let (result, sed_err) = execute_sed(
                    &script,
                    &labels,
                    &content,
                    opts.quiet,
                    ctx.limits.max_loop_iterations,
                    ctx.limits.max_output_size,
                );
                stderr.push_str(&sed_err);
                if let Err(ref e) = ctx.fs.write_file(&path, result.as_bytes()) {
                    stderr.push_str(&format!("sed: {}: {}\n", file, e));
                    has_errors = true;
                }
            }
            CommandResult {
                stderr,
                exit_code: if has_errors { 2 } else { 0 },
                ..Default::default()
            }
        } else {
            let mut stdout = String::new();
            let mut stderr = String::new();
            let mut exit_code = 0;

            if opts.files.is_empty() {
                let (out, err) = execute_sed(
                    &script,
                    &labels,
                    ctx.stdin,
                    opts.quiet,
                    ctx.limits.max_loop_iterations,
                    ctx.limits.max_output_size,
                );
                stdout = out;
                stderr.push_str(&err);
            } else {
                for file in &opts.files {
                    if *file == "-" {
                        let (out, err) = execute_sed(
                            &script,
                            &labels,
                            ctx.stdin,
                            opts.quiet,
                            ctx.limits.max_loop_iterations,
                            ctx.limits.max_output_size,
                        );
                        stdout.push_str(&out);
                        stderr.push_str(&err);
                    } else {
                        let path = resolve_path(file, ctx.cwd);
                        match ctx.fs.read_file(&path) {
                            Ok(bytes) => {
                                let content = String::from_utf8_lossy(&bytes).to_string();
                                let (out, err) = execute_sed(
                                    &script,
                                    &labels,
                                    &content,
                                    opts.quiet,
                                    ctx.limits.max_loop_iterations,
                                    ctx.limits.max_output_size,
                                );
                                stdout.push_str(&out);
                                stderr.push_str(&err);
                            }
                            Err(e) => {
                                stderr.push_str(&format!("sed: {}: {}\n", file, e));
                                exit_code = 2;
                            }
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
}

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

// ── Argument parsing (4f) ────────────────────────────────────────────

fn parse_args(args: &[String]) -> Result<SedOpts<'_>, CommandResult> {
    let mut opts = SedOpts {
        quiet: false,
        in_place: false,
        extended_regex: false,
        scripts: Vec::new(),
        script_files: Vec::new(),
        files: Vec::new(),
    };
    let mut i = 0;
    let mut opts_done = false;

    while i < args.len() {
        let arg = &args[i];
        if opts_done || !arg.starts_with('-') || arg == "-" {
            break;
        }
        match arg.as_str() {
            "--" => {
                opts_done = true;
                i += 1;
                continue;
            }
            "-n" | "--quiet" | "--silent" => opts.quiet = true,
            "-i" | "--in-place" => opts.in_place = true,
            "-E" | "-r" => opts.extended_regex = true,
            "-e" => {
                i += 1;
                if i >= args.len() {
                    return Err(CommandResult {
                        stderr: "sed: option -e requires an argument\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                opts.scripts.push(&args[i]);
            }
            "-f" => {
                i += 1;
                if i >= args.len() {
                    return Err(CommandResult {
                        stderr: "sed: option -f requires an argument\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                opts.script_files.push(&args[i]);
            }
            other => {
                // Handle combined flags like -ne, -nE, etc.
                if other.starts_with('-') && other.len() > 1 {
                    let chars: Vec<char> = other[1..].chars().collect();
                    let mut j = 0;
                    while j < chars.len() {
                        match chars[j] {
                            'n' => opts.quiet = true,
                            'i' => opts.in_place = true,
                            'E' | 'r' => opts.extended_regex = true,
                            'e' => {
                                // Rest of this arg or next arg is the script
                                if j + 1 < chars.len() {
                                    let start = 1 + other[1..]
                                        .char_indices()
                                        .nth(j + 1)
                                        .map(|(idx, _)| idx)
                                        .unwrap_or(other.len() - 1);
                                    opts.scripts.push(&args[i][start..]);
                                } else {
                                    i += 1;
                                    if i >= args.len() {
                                        return Err(CommandResult {
                                            stderr: "sed: option -e requires an argument\n"
                                                .to_string(),
                                            exit_code: 2,
                                            ..Default::default()
                                        });
                                    }
                                    opts.scripts.push(&args[i]);
                                }
                                j = chars.len(); // consumed
                                continue;
                            }
                            'f' => {
                                i += 1;
                                if i >= args.len() {
                                    return Err(CommandResult {
                                        stderr: "sed: option -f requires an argument\n".to_string(),
                                        exit_code: 2,
                                        ..Default::default()
                                    });
                                }
                                opts.script_files.push(&args[i]);
                                j = chars.len();
                                continue;
                            }
                            _ => {
                                return Err(CommandResult {
                                    stderr: format!("sed: unknown option -- '{}'\n", chars[j]),
                                    exit_code: 2,
                                    ..Default::default()
                                });
                            }
                        }
                        j += 1;
                    }
                } else {
                    break;
                }
            }
        }
        i += 1;
    }

    // Remaining args: first is script (if no -e/-f), rest are files
    if opts.scripts.is_empty() && opts.script_files.is_empty() {
        if i >= args.len() {
            return Err(CommandResult {
                stderr: "sed: no script specified\n".to_string(),
                exit_code: 2,
                ..Default::default()
            });
        }
        opts.scripts.push(&args[i]);
        i += 1;
    }
    while i < args.len() {
        opts.files.push(&args[i]);
        i += 1;
    }

    Ok(opts)
}

// ── Script parsing (4b) ─────────────────────────────────────────────

fn parse_script(text: &str, extended: bool) -> Result<SedScript, String> {
    let mut chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    parse_commands(&mut chars, &mut pos, extended, false)
}

fn skip_whitespace(chars: &[char], pos: &mut usize) {
    while *pos < chars.len() && (chars[*pos] == ' ' || chars[*pos] == '\t') {
        *pos += 1;
    }
}

fn parse_commands(
    chars: &mut Vec<char>,
    pos: &mut usize,
    extended: bool,
    in_group: bool,
) -> Result<SedScript, String> {
    let mut commands: SedScript = Vec::new();

    loop {
        // Skip whitespace, semicolons, newlines
        while *pos < chars.len()
            && (chars[*pos] == ';'
                || chars[*pos] == '\n'
                || chars[*pos] == ' '
                || chars[*pos] == '\t')
        {
            *pos += 1;
        }
        if *pos >= chars.len() {
            break;
        }
        if in_group && chars[*pos] == '}' {
            *pos += 1;
            return Ok(commands);
        }

        // Parse address
        let range = parse_range(chars, pos, extended)?;

        skip_whitespace(chars, pos);

        // Check for negation
        let negated = if *pos < chars.len() && chars[*pos] == '!' {
            *pos += 1;
            skip_whitespace(chars, pos);
            true
        } else {
            false
        };

        if *pos >= chars.len() {
            if range.is_some() {
                return Err("unexpected end of script after address".to_string());
            }
            break;
        }

        // Parse command character
        let cmd = parse_single_command(chars, pos, extended)?;

        commands.push(AddressedCmd {
            range,
            negated,
            cmd,
        });
    }

    if in_group {
        return Err("unterminated `{`".to_string());
    }
    Ok(commands)
}

fn parse_range(
    chars: &[char],
    pos: &mut usize,
    extended: bool,
) -> Result<Option<SedRange>, String> {
    let first = match parse_address(chars, pos, extended)? {
        Some(addr) => addr,
        None => return Ok(None),
    };

    skip_whitespace(chars, pos);

    if *pos < chars.len() && chars[*pos] == ',' {
        *pos += 1;
        skip_whitespace(chars, pos);
        let second = parse_address(chars, pos, extended)?
            .ok_or_else(|| "expected address after ','".to_string())?;
        Ok(Some(SedRange::Range(first, second)))
    } else {
        Ok(Some(SedRange::Single(first)))
    }
}

fn parse_address(
    chars: &[char],
    pos: &mut usize,
    extended: bool,
) -> Result<Option<SedAddress>, String> {
    if *pos >= chars.len() {
        return Ok(None);
    }

    if chars[*pos] == '$' {
        *pos += 1;
        return Ok(Some(SedAddress::Last));
    }

    if chars[*pos] == '/' || chars[*pos] == '\\' {
        let delim = if chars[*pos] == '\\' {
            *pos += 1;
            if *pos >= chars.len() {
                return Err("expected delimiter after '\\'".to_string());
            }
            let d = chars[*pos];
            *pos += 1;
            d
        } else {
            let d = chars[*pos];
            *pos += 1;
            d
        };
        let pattern = read_delimited(chars, pos, delim)?;
        let ere = if extended {
            pattern.clone()
        } else {
            bre_to_ere(&pattern)
        };
        let re = Regex::new(&ere).map_err(|e| format!("invalid regex '{}': {}", pattern, e))?;
        return Ok(Some(SedAddress::Regex(re)));
    }

    if chars[*pos].is_ascii_digit() {
        let start = *pos;
        while *pos < chars.len() && chars[*pos].is_ascii_digit() {
            *pos += 1;
        }
        let num: usize = chars[start..*pos]
            .iter()
            .collect::<String>()
            .parse()
            .map_err(|_| "invalid line number".to_string())?;

        // Check for step: N~S
        if *pos < chars.len() && chars[*pos] == '~' {
            *pos += 1;
            let step_start = *pos;
            while *pos < chars.len() && chars[*pos].is_ascii_digit() {
                *pos += 1;
            }
            if *pos == step_start {
                return Err("expected step number after '~'".to_string());
            }
            let step: usize = chars[step_start..*pos]
                .iter()
                .collect::<String>()
                .parse()
                .map_err(|_| "invalid step number".to_string())?;
            return Ok(Some(SedAddress::Step(num, step)));
        }

        return Ok(Some(SedAddress::LineNumber(num)));
    }

    Ok(None)
}

fn read_delimited(chars: &[char], pos: &mut usize, delim: char) -> Result<String, String> {
    let mut result = String::new();
    while *pos < chars.len() && chars[*pos] != delim {
        if chars[*pos] == '\\' && *pos + 1 < chars.len() {
            if chars[*pos + 1] == delim {
                result.push(delim);
                *pos += 2;
            } else {
                result.push('\\');
                result.push(chars[*pos + 1]);
                *pos += 2;
            }
        } else {
            result.push(chars[*pos]);
            *pos += 1;
        }
    }
    if *pos < chars.len() && chars[*pos] == delim {
        *pos += 1;
    }
    Ok(result)
}

fn parse_single_command(
    chars: &mut Vec<char>,
    pos: &mut usize,
    extended: bool,
) -> Result<SedCmd, String> {
    if *pos >= chars.len() {
        return Err("expected command".to_string());
    }

    let ch = chars[*pos];
    *pos += 1;

    match ch {
        'd' => Ok(SedCmd::Delete),
        'p' => Ok(SedCmd::Print),
        'q' => Ok(SedCmd::Quit),
        'h' => Ok(SedCmd::HoldGet),
        'H' => Ok(SedCmd::HoldAppend),
        'g' => Ok(SedCmd::PatternGet),
        'G' => Ok(SedCmd::PatternAppend),
        'x' => Ok(SedCmd::Exchange),
        '=' => Ok(SedCmd::LineNumber),
        'n' => Ok(SedCmd::Next),
        'N' => Ok(SedCmd::NextAppend),
        's' => parse_substitute(chars, pos, extended),
        'y' => parse_transliterate(chars, pos),
        'a' => parse_text_command(chars, pos),
        'i' => parse_text_command(chars, pos),
        'c' => parse_text_command(chars, pos),
        ':' => {
            skip_whitespace(chars, pos);
            let label = read_label(chars, pos);
            if label.is_empty() {
                return Err("missing label after ':'".to_string());
            }
            Ok(SedCmd::Label(label))
        }
        'b' => {
            skip_whitespace(chars, pos);
            let label = read_label(chars, pos);
            if label.is_empty() {
                Ok(SedCmd::Branch(None))
            } else {
                Ok(SedCmd::Branch(Some(label)))
            }
        }
        't' => {
            skip_whitespace(chars, pos);
            let label = read_label(chars, pos);
            if label.is_empty() {
                Ok(SedCmd::BranchIfSubstituted(None))
            } else {
                Ok(SedCmd::BranchIfSubstituted(Some(label)))
            }
        }
        'T' => {
            skip_whitespace(chars, pos);
            let label = read_label(chars, pos);
            if label.is_empty() {
                Ok(SedCmd::BranchIfNotSubstituted(None))
            } else {
                Ok(SedCmd::BranchIfNotSubstituted(Some(label)))
            }
        }
        '{' => {
            let group = parse_commands(chars, pos, extended, true)?;
            Ok(SedCmd::CommandGroup(group))
        }
        '#' => {
            // Comment: skip to end of line
            while *pos < chars.len() && chars[*pos] != '\n' {
                *pos += 1;
            }
            Ok(SedCmd::Noop)
        }
        other => Err(format!("unknown command: '{}'", other)),
    }
}

fn read_label(chars: &[char], pos: &mut usize) -> String {
    let mut label = String::new();
    while *pos < chars.len()
        && chars[*pos] != ';'
        && chars[*pos] != '\n'
        && chars[*pos] != '}'
        && chars[*pos] != ' '
        && chars[*pos] != '\t'
    {
        label.push(chars[*pos]);
        *pos += 1;
    }
    label
}

fn parse_text_command(chars: &[char], pos: &mut usize) -> Result<SedCmd, String> {
    // a\text, i\text, c\text — or a text (space after command)
    let cmd_char = chars[*pos - 1]; // we already advanced past it
    let mut text = String::new();

    if *pos < chars.len() && chars[*pos] == '\\' {
        *pos += 1;
        // Skip optional newline after backslash
        if *pos < chars.len() && chars[*pos] == '\n' {
            *pos += 1;
        }
    } else if *pos < chars.len() && chars[*pos] == ' ' {
        *pos += 1;
    }

    // Read to end of line (or end of input)
    while *pos < chars.len() && chars[*pos] != '\n' && chars[*pos] != ';' {
        text.push(chars[*pos]);
        *pos += 1;
    }

    match cmd_char {
        'a' => Ok(SedCmd::Append(text)),
        'i' => Ok(SedCmd::Insert(text)),
        'c' => Ok(SedCmd::Change(text)),
        _ => unreachable!(),
    }
}

fn parse_substitute(chars: &[char], pos: &mut usize, extended: bool) -> Result<SedCmd, String> {
    if *pos >= chars.len() {
        return Err("unterminated `s` command".to_string());
    }
    let delim = chars[*pos];
    *pos += 1;

    let pattern = read_delimited(chars, pos, delim)?;
    let raw_replacement = read_delimited(chars, pos, delim)?;

    // Parse flags
    let mut flags = SubstituteFlags {
        global: false,
        case_insensitive: false,
        print: false,
        nth: None,
    };
    while *pos < chars.len() && chars[*pos] != ';' && chars[*pos] != '\n' && chars[*pos] != '}' {
        match chars[*pos] {
            'g' => flags.global = true,
            'i' | 'I' => flags.case_insensitive = true,
            'p' => flags.print = true,
            c if c.is_ascii_digit() => {
                let start = *pos;
                while *pos < chars.len() && chars[*pos].is_ascii_digit() {
                    *pos += 1;
                }
                let n: usize = chars[start..*pos]
                    .iter()
                    .collect::<String>()
                    .parse()
                    .map_err(|_| "invalid flag number".to_string())?;
                flags.nth = Some(n);
                continue;
            }
            ' ' | '\t' => {
                *pos += 1;
                continue;
            }
            _ => break,
        }
        *pos += 1;
    }

    // Build regex: BRE→ERE unless extended
    let ere = if extended {
        pattern.clone()
    } else {
        bre_to_ere(&pattern)
    };

    let full_pattern = if flags.case_insensitive {
        format!("(?i){}", ere)
    } else {
        ere
    };

    let regex =
        Regex::new(&full_pattern).map_err(|e| format!("invalid regex '{}': {}", pattern, e))?;

    // Translate replacement: \1-\9 → ${1}-${9}, & → $0, \n → newline
    let replacement = translate_replacement(&raw_replacement);

    Ok(SedCmd::Substitute {
        regex,
        replacement,
        flags,
    })
}

/// Translate sed replacement to regex crate replacement format.
/// `\1`-`\9` → `${1}`-`${9}`, `&` → `$0`, `\n` → newline, `\\` → `\`
fn translate_replacement(raw: &str) -> String {
    let mut result = String::with_capacity(raw.len());
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            match chars[i + 1] {
                c @ '1'..='9' => {
                    result.push_str(&format!("${{{}}}", c));
                    i += 2;
                }
                'n' => {
                    result.push('\n');
                    i += 2;
                }
                '\\' => {
                    result.push('\\');
                    i += 2;
                }
                '&' => {
                    // Literal &
                    result.push('&');
                    i += 2;
                }
                _ => {
                    result.push('\\');
                    result.push(chars[i + 1]);
                    i += 2;
                }
            }
        } else if chars[i] == '&' {
            result.push_str("${0}");
            i += 1;
        } else if chars[i] == '$' {
            result.push_str("$$");
            i += 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }
    result
}

fn parse_transliterate(chars: &[char], pos: &mut usize) -> Result<SedCmd, String> {
    if *pos >= chars.len() {
        return Err("unterminated `y` command".to_string());
    }
    let delim = chars[*pos];
    *pos += 1;

    let src_str = read_delimited(chars, pos, delim)?;
    let dst_str = read_delimited(chars, pos, delim)?;

    let src: Vec<char> = src_str.chars().collect();
    let dst: Vec<char> = dst_str.chars().collect();

    if src.len() != dst.len() {
        return Err(format!(
            "`y` command strings have different lengths ({} vs {})",
            src.len(),
            dst.len()
        ));
    }

    Ok(SedCmd::Transliterate(src, dst))
}

// ── Label collection ─────────────────────────────────────────────────

fn collect_labels(script: &SedScript) -> std::collections::HashMap<String, usize> {
    let mut map = std::collections::HashMap::new();
    collect_labels_recursive(script, &mut map);
    map
}

fn collect_labels_recursive(
    script: &SedScript,
    map: &mut std::collections::HashMap<String, usize>,
) {
    for (idx, entry) in script.iter().enumerate() {
        if let SedCmd::Label(name) = &entry.cmd {
            map.insert(name.clone(), idx);
        }
        // Labels inside groups use group-relative indices which don't work
        // with top-level branching, so we skip recursive collection.
    }
}

// ── Execution engine (4c) ────────────────────────────────────────────

struct SedState {
    pattern_space: String,
    hold_space: String,
    line_number: usize,
    last_line: bool,
    last_sub_success: bool,
    output: String,
    stderr: String,
    quit: bool,
    deleted: bool,
    /// Per-range state: maps script index to whether range is currently active.
    range_active: std::collections::HashMap<usize, bool>,
    /// Text to append after the pattern space is output.
    append_queue: Vec<String>,
    cycle_count: usize,
    max_cycles: usize,
    max_output_size: usize,
    output_truncated: bool,
}

impl SedState {
    fn push_output(&mut self, s: &str) {
        if self.output.len() > self.max_output_size {
            if !self.output_truncated {
                self.stderr.push_str("sed: output size limit exceeded\n");
                self.output_truncated = true;
            }
            return;
        }
        self.output.push_str(s);
    }

    fn push_output_char(&mut self, c: char) {
        if self.output.len() > self.max_output_size {
            if !self.output_truncated {
                self.stderr.push_str("sed: output size limit exceeded\n");
                self.output_truncated = true;
            }
            return;
        }
        self.output.push(c);
    }
}

fn execute_sed(
    script: &SedScript,
    labels: &std::collections::HashMap<String, usize>,
    input: &str,
    quiet: bool,
    max_cycles: usize,
    max_output_size: usize,
) -> (String, String) {
    let lines: Vec<&str> = input.split('\n').collect();
    // Remove trailing empty element from trailing newline
    let total = if !lines.is_empty() && lines.last() == Some(&"") {
        lines.len() - 1
    } else {
        lines.len()
    };

    let mut state = SedState {
        pattern_space: String::new(),
        hold_space: String::new(),
        line_number: 0,
        last_line: false,
        last_sub_success: false,
        output: String::new(),
        stderr: String::new(),
        quit: false,
        deleted: false,
        range_active: std::collections::HashMap::new(),
        append_queue: Vec::new(),
        cycle_count: 0,
        max_cycles,
        max_output_size,
        output_truncated: false,
    };

    let mut line_idx = 0;
    while line_idx < total {
        state.line_number += 1;
        state.last_line = line_idx + 1 >= total;
        state.pattern_space = lines[line_idx].to_string();
        state.deleted = false;
        state.last_sub_success = false;

        execute_commands(
            script,
            labels,
            &mut state,
            quiet,
            &lines,
            total,
            &mut line_idx,
        );

        if state.quit {
            if !state.deleted && !quiet {
                state.push_output(&state.pattern_space.clone());
                state.push_output_char('\n');
            }
            let queued: Vec<String> = state.append_queue.drain(..).collect();
            for text in queued {
                state.push_output(&text);
                state.push_output_char('\n');
            }
            break;
        }

        if !state.deleted && !quiet {
            state.push_output(&state.pattern_space.clone());
            state.push_output_char('\n');
        }
        let queued: Vec<String> = state.append_queue.drain(..).collect();
        for text in queued {
            state.push_output(&text);
            state.push_output_char('\n');
        }

        line_idx += 1;
    }

    (state.output, state.stderr)
}

/// Execute the command list. Returns a flow control signal.
fn execute_commands(
    script: &SedScript,
    labels: &std::collections::HashMap<String, usize>,
    state: &mut SedState,
    quiet: bool,
    lines: &[&str],
    total: usize,
    line_idx: &mut usize,
) {
    let mut ip = 0; // instruction pointer
    while ip < script.len() {
        state.cycle_count += 1;
        if state.cycle_count > state.max_cycles {
            state.stderr.push_str("sed: cycle limit exceeded\n");
            state.quit = true;
            return;
        }

        if state.quit || state.deleted {
            return;
        }

        let entry = &script[ip];
        let matches = address_matches(&entry.range, state, ip);
        let should_run = if entry.negated { !matches } else { matches };

        if should_run {
            match execute_one(&entry.cmd, labels, state, quiet, lines, total, line_idx) {
                Flow::Continue => {}
                Flow::Break => return,
                Flow::BranchTo(label) => {
                    if let Some(&target) = labels.get(&label) {
                        ip = target;
                        continue;
                    }
                    // Label not found: treat as branch to end
                    return;
                }
                Flow::BranchEnd => return,
            }
        }

        ip += 1;
    }
}

enum Flow {
    Continue,
    Break,
    BranchTo(String),
    BranchEnd,
}

fn address_matches(range: &Option<SedRange>, state: &mut SedState, ip: usize) -> bool {
    match range {
        None => true,
        Some(SedRange::Single(addr)) => addr_matches(addr, state),
        Some(SedRange::Range(start, end)) => {
            let active = *state.range_active.get(&ip).unwrap_or(&false);
            if active {
                // Check if end address is reached
                let end_match = addr_matches(end, state);
                if end_match {
                    state.range_active.insert(ip, false);
                }
                true
            } else {
                // Check if start address matches
                let start_match = addr_matches(start, state);
                if start_match {
                    // Activate range. Check if end also matches on the same line.
                    let end_match = addr_matches(end, state);
                    if !end_match {
                        state.range_active.insert(ip, true);
                    }
                    true
                } else {
                    false
                }
            }
        }
    }
}

fn addr_matches(addr: &SedAddress, state: &SedState) -> bool {
    match addr {
        SedAddress::LineNumber(n) => state.line_number == *n,
        SedAddress::Last => state.last_line,
        SedAddress::Regex(re) => re.is_match(&state.pattern_space),
        SedAddress::Step(first, step) => {
            if *step == 0 {
                state.line_number == *first
            } else {
                state.line_number >= *first && (state.line_number - *first).is_multiple_of(*step)
            }
        }
    }
}

fn execute_one(
    cmd: &SedCmd,
    labels: &std::collections::HashMap<String, usize>,
    state: &mut SedState,
    quiet: bool,
    lines: &[&str],
    total: usize,
    line_idx: &mut usize,
) -> Flow {
    match cmd {
        SedCmd::Noop => Flow::Continue,

        SedCmd::Substitute {
            regex,
            replacement,
            flags,
        } => {
            let old = state.pattern_space.clone();
            if flags.global {
                state.pattern_space = regex.replace_all(&old, replacement.as_str()).to_string();
            } else if let Some(nth) = flags.nth {
                // Replace the nth match only
                let mut count = 0;
                let mut result = String::new();
                let mut last_end = 0;
                for mat in regex.find_iter(&old) {
                    count += 1;
                    if count == nth {
                        result.push_str(&old[last_end..mat.start()]);
                        // Use regex.replace on just this match for proper group expansion
                        let replaced = regex.replace(mat.as_str(), replacement.as_str());
                        result.push_str(&replaced);
                        last_end = mat.end();
                        // Append remainder
                        result.push_str(&old[last_end..]);
                        state.pattern_space = result;
                        state.last_sub_success = true;
                        if flags.print {
                            state.push_output(&state.pattern_space.clone());
                            state.push_output_char('\n');
                        }
                        return Flow::Continue;
                    }
                }
                // If nth not found, no replacement
                state.last_sub_success = false;
            } else {
                state.pattern_space = regex.replace(&old, replacement.as_str()).to_string();
            }
            state.last_sub_success = state.pattern_space != old;
            if state.last_sub_success && flags.print {
                state.push_output(&state.pattern_space.clone());
                state.push_output_char('\n');
            }
            Flow::Continue
        }

        SedCmd::Delete => {
            state.deleted = true;
            Flow::Break
        }

        SedCmd::Print => {
            state.push_output(&state.pattern_space.clone());
            state.push_output_char('\n');
            Flow::Continue
        }

        SedCmd::Quit => {
            state.quit = true;
            Flow::Break
        }

        SedCmd::Append(text) => {
            state.append_queue.push(text.clone());
            Flow::Continue
        }

        SedCmd::Insert(text) => {
            state.push_output(text);
            state.push_output_char('\n');
            Flow::Continue
        }

        SedCmd::Change(text) => {
            state.pattern_space = text.clone();
            state.push_output(text);
            state.push_output_char('\n');
            state.deleted = true;
            Flow::Break
        }

        SedCmd::Label(_) => Flow::Continue,

        SedCmd::Branch(label) => match label {
            Some(l) => Flow::BranchTo(l.clone()),
            None => Flow::BranchEnd,
        },

        SedCmd::BranchIfSubstituted(label) => {
            if state.last_sub_success {
                state.last_sub_success = false;
                match label {
                    Some(l) => Flow::BranchTo(l.clone()),
                    None => Flow::BranchEnd,
                }
            } else {
                Flow::Continue
            }
        }

        SedCmd::BranchIfNotSubstituted(label) => {
            if !state.last_sub_success {
                match label {
                    Some(l) => Flow::BranchTo(l.clone()),
                    None => Flow::BranchEnd,
                }
            } else {
                state.last_sub_success = false;
                Flow::Continue
            }
        }

        SedCmd::HoldGet => {
            state.hold_space = state.pattern_space.clone();
            Flow::Continue
        }

        SedCmd::HoldAppend => {
            state.hold_space.push('\n');
            state.hold_space.push_str(&state.pattern_space);
            Flow::Continue
        }

        SedCmd::PatternGet => {
            state.pattern_space = state.hold_space.clone();
            Flow::Continue
        }

        SedCmd::PatternAppend => {
            state.pattern_space.push('\n');
            state.pattern_space.push_str(&state.hold_space);
            Flow::Continue
        }

        SedCmd::Exchange => {
            std::mem::swap(&mut state.pattern_space, &mut state.hold_space);
            Flow::Continue
        }

        SedCmd::LineNumber => {
            state.push_output(&state.line_number.to_string());
            state.push_output_char('\n');
            Flow::Continue
        }

        SedCmd::Next => {
            // Print pattern space (unless -n), then read next line
            if !quiet {
                state.push_output(&state.pattern_space.clone());
                state.push_output_char('\n');
            }
            *line_idx += 1;
            state.line_number += 1;
            if *line_idx >= total {
                // No more input: end processing
                state.deleted = true;
                return Flow::Break;
            }
            state.last_line = *line_idx + 1 >= total;
            state.pattern_space = lines[*line_idx].to_string();
            Flow::Continue
        }

        SedCmd::NextAppend => {
            // Append next line to pattern space with \n
            *line_idx += 1;
            state.line_number += 1;
            if *line_idx >= total {
                // No more input: write pattern space (unless -n) and exit
                if !quiet {
                    state.push_output(&state.pattern_space.clone());
                    state.push_output_char('\n');
                }
                state.deleted = true;
                return Flow::Break;
            }
            state.last_line = *line_idx + 1 >= total;
            state.pattern_space.push('\n');
            state.pattern_space.push_str(lines[*line_idx]);
            Flow::Continue
        }

        SedCmd::Transliterate(src, dst) => {
            let mut new = String::with_capacity(state.pattern_space.len());
            for ch in state.pattern_space.chars() {
                if let Some(idx) = src.iter().position(|&c| c == ch) {
                    new.push(dst[idx]);
                } else {
                    new.push(ch);
                }
            }
            state.pattern_space = new;
            Flow::Continue
        }

        SedCmd::CommandGroup(sub_cmds) => {
            execute_commands(sub_cmds, labels, state, quiet, lines, total, line_idx);
            if state.quit || state.deleted {
                Flow::Break
            } else {
                Flow::Continue
            }
        }
    }
}

// ── Address range tracking ───────────────────────────────────────────
// For proper range tracking with regex addresses we'd need state per range.
// The current implementation handles line-number and simple ranges correctly.

// ── Tests (4g) ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandContext, CommandResult, VirtualCommand};
    use crate::interpreter::ExecutionLimits;
    use crate::network::NetworkPolicy;
    use crate::vfs::{InMemoryFs, VirtualFs};
    use std::collections::HashMap;
    use std::path::Path;
    use std::sync::Arc;

    fn run_sed(args: &[&str], stdin: &str) -> CommandResult {
        let fs = InMemoryFs::new();
        let env = HashMap::new();
        let limits = ExecutionLimits::default();
        let ctx = CommandContext {
            fs: &fs,
            cwd: "/",
            env: &env,
            stdin,
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        SedCommand.execute(&args, &ctx)
    }

    fn run_sed_with_fs(args: &[&str], stdin: &str, fs: &Arc<InMemoryFs>) -> CommandResult {
        let env = HashMap::new();
        let limits = ExecutionLimits::default();
        let ctx = CommandContext {
            fs: fs.as_ref(),
            cwd: "/",
            env: &env,
            stdin,
            limits: &limits,
            network_policy: &NetworkPolicy::default(),
            exec: None,
        };
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        SedCommand.execute(&args, &ctx)
    }

    // ── Basic substitution ───────────────────────────────────────────

    #[test]
    fn substitute_basic() {
        let r = run_sed(&["s/old/new/"], "old text\n");
        assert_eq!(r.stdout, "new text\n");
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn substitute_global() {
        let r = run_sed(&["s/a/b/g"], "aaa\n");
        assert_eq!(r.stdout, "bbb\n");
    }

    #[test]
    fn substitute_case_insensitive() {
        let r = run_sed(&["s/hello/world/i"], "Hello there\n");
        assert_eq!(r.stdout, "world there\n");
    }

    #[test]
    fn substitute_no_match_passthrough() {
        let r = run_sed(&["s/xyz/abc/"], "hello\n");
        assert_eq!(r.stdout, "hello\n");
    }

    // ── Delete ───────────────────────────────────────────────────────

    #[test]
    fn delete_line_3() {
        let r = run_sed(&["3d"], "a\nb\nc\nd\n");
        assert_eq!(r.stdout, "a\nb\nd\n");
    }

    #[test]
    fn delete_pattern_match() {
        let r = run_sed(&["/banana/d"], "apple\nbanana\ncherry\n");
        assert_eq!(r.stdout, "apple\ncherry\n");
    }

    // ── Print ────────────────────────────────────────────────────────

    #[test]
    fn print_lines_1_to_5() {
        let r = run_sed(&["1,5p"], "a\nb\nc\nd\ne\nf\n");
        // Without -n, each line 1-5 is printed twice (auto + p), line 6 once
        assert_eq!(r.stdout, "a\na\nb\nb\nc\nc\nd\nd\ne\ne\nf\n");
    }

    #[test]
    fn quiet_with_print() {
        let r = run_sed(&["-n", "2p"], "a\nb\nc\n");
        assert_eq!(r.stdout, "b\n");
    }

    // ── Quit ─────────────────────────────────────────────────────────

    #[test]
    fn quit_after_first_line() {
        let r = run_sed(&["q"], "first\nsecond\nthird\n");
        assert_eq!(r.stdout, "first\n");
    }

    // ── Text insertion commands ──────────────────────────────────────

    #[test]
    fn append_text() {
        let r = run_sed(&["1a\\appended"], "line1\nline2\n");
        assert_eq!(r.stdout, "line1\nappended\nline2\n");
    }

    #[test]
    fn insert_text() {
        let r = run_sed(&["1i\\inserted"], "line1\nline2\n");
        assert_eq!(r.stdout, "inserted\nline1\nline2\n");
    }

    #[test]
    fn change_text() {
        let r = run_sed(&["2c\\changed"], "line1\nline2\nline3\n");
        assert_eq!(r.stdout, "line1\nchanged\nline3\n");
    }

    // ── Transliterate ────────────────────────────────────────────────

    #[test]
    fn transliterate_basic() {
        let r = run_sed(&["y/abc/ABC/"], "abcdef\n");
        assert_eq!(r.stdout, "ABCdef\n");
    }

    // ── Line number ──────────────────────────────────────────────────

    #[test]
    fn print_line_number() {
        let r = run_sed(&["="], "a\nb\n");
        assert_eq!(r.stdout, "1\na\n2\nb\n");
    }

    // ── Next line operations ─────────────────────────────────────────

    #[test]
    fn next_line() {
        // n skips the current line's remaining commands and prints it, loads next
        let r = run_sed(&["-n", "{n;p}"], "a\nb\nc\n");
        assert_eq!(r.stdout, "b\n");
    }

    #[test]
    fn next_append() {
        // N appends next line with \n
        let r = run_sed(&["-n", "{N;p}"], "a\nb\nc\n");
        assert_eq!(r.stdout, "a\nb\n");
    }

    // ── Command grouping ─────────────────────────────────────────────

    #[test]
    fn command_group() {
        let r = run_sed(&["2,3{ s/a/b/; s/c/d/ }"], "ac\nac\nac\nac\n");
        assert_eq!(r.stdout, "ac\nbd\nbd\nac\n");
    }

    // ── In-place editing ─────────────────────────────────────────────

    #[test]
    fn in_place_edit() {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(Path::new("/test.txt"), b"hello world\n")
            .unwrap();
        let r = run_sed_with_fs(&["-i", "s/world/earth/", "/test.txt"], "", &fs);
        assert_eq!(r.exit_code, 0);
        let content = fs.read_file(Path::new("/test.txt")).unwrap();
        assert_eq!(String::from_utf8_lossy(&content), "hello earth\n");
    }

    // ── Hold space ───────────────────────────────────────────────────

    #[test]
    fn hold_space_join_lines() {
        // sed -n 'H;${x;s/\n/ /g;p}' — join all lines with spaces
        let r = run_sed(&["-n", "H;${x;s/\\n/ /g;p}"], "one\ntwo\nthree\n");
        assert_eq!(r.stdout, " one two three\n");
    }

    // ── Branching ────────────────────────────────────────────────────

    #[test]
    fn branch_join_lines() {
        // sed ':a;N;$!ba;s/\n/ /g' — join all lines with spaces using labels
        let r = run_sed(&[":a;N;$!ba;s/\\n/ /g"], "one\ntwo\nthree\n");
        assert_eq!(r.stdout, "one two three\n");
    }

    // ── Multiple -e expressions ──────────────────────────────────────

    #[test]
    fn multiple_expressions() {
        let r = run_sed(&["-e", "s/hello/world/", "-e", "s/world/earth/"], "hello\n");
        assert_eq!(r.stdout, "earth\n");
    }

    // ── Alternate delimiters ─────────────────────────────────────────

    #[test]
    fn alternate_delimiter() {
        let r = run_sed(&["s|/path/old|/path/new|"], "/path/old/file\n");
        assert_eq!(r.stdout, "/path/new/file\n");
    }

    // ── Backreferences ───────────────────────────────────────────────

    #[test]
    fn backreferences_bre() {
        // BRE: \(foo\)\(bar\) → groups, \2\1 → swap
        let r = run_sed(&[r"s/\(foo\)\(bar\)/\2\1/"], "foobar\n");
        assert_eq!(r.stdout, "barfoo\n");
    }

    // ── Pipeline ─────────────────────────────────────────────────────

    #[test]
    fn pipeline_stdin() {
        let r = run_sed(&["s/world/earth/"], "hello world\n");
        assert_eq!(r.stdout, "hello earth\n");
    }

    // ── Address ranges ───────────────────────────────────────────────

    #[test]
    fn address_range_substitute() {
        let r = run_sed(&["2,4s/a/b/g"], "aaa\naaa\naaa\naaa\naaa\n");
        assert_eq!(r.stdout, "aaa\nbbb\nbbb\nbbb\naaa\n");
    }

    // ── Extended regex ───────────────────────────────────────────────

    #[test]
    fn extended_regex_flag() {
        // With -E, bare parens are groups (no need for \( \))
        let r = run_sed(&["-E", "s/(foo)(bar)/\\2\\1/"], "foobar\n");
        assert_eq!(r.stdout, "barfoo\n");
    }

    // ── Negated address (using $!) ───────────────────────────────────

    #[test]
    fn last_line_address() {
        let r = run_sed(&["$d"], "a\nb\nc\n");
        assert_eq!(r.stdout, "a\nb\n");
    }

    // ── Regex address ────────────────────────────────────────────────

    #[test]
    fn regex_address_substitute() {
        let r = run_sed(&["/^#/d"], "# comment\ncode\n# another\n");
        assert_eq!(r.stdout, "code\n");
    }

    // ── Multiple files ───────────────────────────────────────────────

    #[test]
    fn multiple_files() {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(Path::new("/a.txt"), b"hello\n").unwrap();
        fs.write_file(Path::new("/b.txt"), b"world\n").unwrap();
        let r = run_sed_with_fs(&["s/hello/hi/;s/world/earth/", "/a.txt", "/b.txt"], "", &fs);
        assert_eq!(r.stdout, "hi\nearth\n");
    }

    // ── Whole-match replacement with & ───────────────────────────────

    #[test]
    fn whole_match_ampersand() {
        let r = run_sed(&["s/[0-9]\\{1,\\}/[&]/g"], "line 42 and 7\n");
        assert_eq!(r.stdout, "line [42] and [7]\n");
    }

    // ── Script from file (-f) ────────────────────────────────────────

    #[test]
    fn script_from_file() {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(Path::new("/script.sed"), b"s/old/new/\n")
            .unwrap();
        fs.write_file(Path::new("/input.txt"), b"old text\n")
            .unwrap();
        let r = run_sed_with_fs(&["-f", "/script.sed", "/input.txt"], "", &fs);
        assert_eq!(r.stdout, "new text\n");
    }

    // ── Step address ─────────────────────────────────────────────────

    #[test]
    fn step_address() {
        // 0~2 matches every 2nd line starting from line 2 (0 means all matching step from start)
        // 1~2 matches lines 1, 3, 5, ...
        let r = run_sed(&["1~2d"], "a\nb\nc\nd\ne\n");
        assert_eq!(r.stdout, "b\nd\n");
    }

    #[test]
    fn step_address_delete_even_lines() {
        // 0~2 matches lines 2, 4, 6, ... (every 2nd line from start)
        let r = run_sed(&["0~2d"], "a\nb\nc\nd\ne\nf\n");
        assert_eq!(r.stdout, "a\nc\ne\n");
    }

    #[test]
    fn step_address_print_every_third() {
        // 1~3 matches lines 1, 4, 7, ...
        let r = run_sed(&["-n", "1~3p"], "a\nb\nc\nd\ne\nf\ng\n");
        assert_eq!(r.stdout, "a\nd\ng\n");
    }

    #[test]
    fn step_address_zero_step() {
        // N~0 matches only line N
        let r = run_sed(&["3~0d"], "a\nb\nc\nd\ne\n");
        assert_eq!(r.stdout, "a\nb\nd\ne\n");
    }

    // ── Substitute with print flag ───────────────────────────────────

    #[test]
    fn substitute_with_print_flag() {
        let r = run_sed(&["-n", "s/hello/world/p"], "hello\nbye\n");
        assert_eq!(r.stdout, "world\n");
    }

    // ── Exchange command ─────────────────────────────────────────────

    #[test]
    fn exchange_hold_pattern() {
        let r = run_sed(&["-n", "1{h;d};2{x;p}"], "first\nsecond\nthird\n");
        assert_eq!(r.stdout, "first\n");
    }

    // ── Translate replacement special chars ──────────────────────────

    #[test]
    fn translate_replacement_groups() {
        assert_eq!(translate_replacement(r"\1"), "${1}");
        assert_eq!(translate_replacement(r"\2"), "${2}");
        assert_eq!(translate_replacement("&"), "${0}");
        assert_eq!(translate_replacement(r"\n"), "\n");
        assert_eq!(translate_replacement(r"\\"), "\\");
    }

    // ── Stdin via - ──────────────────────────────────────────────────

    #[test]
    fn stdin_dash_argument() {
        let r = run_sed(&["s/a/b/", "-"], "aaa\n");
        assert_eq!(r.stdout, "baa\n");
    }

    // ── Empty input ──────────────────────────────────────────────────

    #[test]
    fn empty_input() {
        let r = run_sed(&["s/a/b/"], "");
        assert_eq!(r.stdout, "");
    }

    // ── No script error ──────────────────────────────────────────────

    #[test]
    fn no_script_error() {
        let r = run_sed(&[], "hello\n");
        assert_eq!(r.exit_code, 2);
        assert!(r.stderr.contains("no script"));
    }

    // ── Regex-based range ────────────────────────────────────────────

    #[test]
    fn regex_range() {
        let r = run_sed(&["/start/,/end/d"], "before\nstart\nmiddle\nend\nafter\n");
        assert_eq!(r.stdout, "before\nafter\n");
    }

    // ── Negated address ──────────────────────────────────────────────

    #[test]
    fn negated_address() {
        let r = run_sed(&["2!d"], "a\nb\nc\n");
        assert_eq!(r.stdout, "b\n");
    }

    // ── T command (branch if NOT substituted) ────────────────────────

    #[test]
    fn branch_if_not_substituted() {
        // T branches if last s/// did NOT succeed
        // "abc": s/x/y/ fails → T branches to end → p prints "abc"
        // "xyz": s/x/y/ succeeds → "yyz" → T does not branch → s/a/b/ no match → p prints "yyz"
        let r = run_sed(&["-n", "s/x/y/;Tend;s/a/b/;:end;p"], "abc\nxyz\n");
        assert_eq!(r.stdout, "abc\nyyz\n");
    }

    // ── Line number + regex mixed range ──────────────────────────────

    #[test]
    fn line_regex_mixed_range() {
        let r = run_sed(&["2,/end/d"], "a\nb\nc\nend\nafter\n");
        assert_eq!(r.stdout, "a\nafter\n");
    }

    // ── Append followed by other commands ────────────────────────────

    #[test]
    fn append_then_substitute() {
        let r = run_sed(&["1a\\APPENDED;s/a/b/"], "aaa\nxxx\n");
        assert_eq!(r.stdout, "baa\nAPPENDED\nxxx\n");
    }

    #[test]
    fn append_with_quiet() {
        let r = run_sed(&["-n", "1a\\APPENDED;1p"], "line1\nline2\n");
        assert_eq!(r.stdout, "line1\nAPPENDED\n");
    }

    // ── Dollar sign in replacement ───────────────────────────────────

    #[test]
    fn dollar_in_replacement() {
        let r = run_sed(&["s/price/$100/"], "price\n");
        assert_eq!(r.stdout, "$100\n");
    }

    // ── Combined flags ───────────────────────────────────────────────

    #[test]
    fn combined_ne_flag() {
        let r = run_sed(&["-ne", "s/a/b/p"], "aaa\nxxx\n");
        assert_eq!(r.stdout, "baa\n");
    }
}
