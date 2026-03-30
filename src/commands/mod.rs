//! Command trait and built-in command implementations.

pub(crate) mod awk;
pub(crate) mod compression;
pub(crate) mod diff_cmd;
pub(crate) mod exec_cmds;
pub(crate) mod file_ops;
pub(crate) mod jq_cmd;
pub(crate) mod navigation;
#[cfg(feature = "network")]
pub(crate) mod net;
pub(crate) mod regex_util;
pub(crate) mod sed;
pub(crate) mod test_cmd;
pub(crate) mod text;
pub(crate) mod utils;

use crate::error::RustBashError;
use crate::interpreter::ExecutionLimits;
use crate::network::NetworkPolicy;
use crate::vfs::VirtualFs;
use std::collections::HashMap;

/// Result of executing a command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    /// Binary output for commands that produce non-text data (e.g. gzip).
    /// When set, pipeline propagation uses this instead of `stdout`.
    pub stdout_bytes: Option<Vec<u8>>,
}

/// Callback type for sub-command execution (e.g. `xargs`, `find -exec`).
pub type ExecCallback<'a> = &'a dyn Fn(&str) -> Result<CommandResult, RustBashError>;

/// Context passed to command execution.
pub struct CommandContext<'a> {
    pub fs: &'a dyn VirtualFs,
    pub cwd: &'a str,
    pub env: &'a HashMap<String, String>,
    pub stdin: &'a str,
    /// Binary input from a previous pipeline stage (e.g. gzip output).
    /// Commands that handle binary input check this first, falling back to `stdin`.
    pub stdin_bytes: Option<&'a [u8]>,
    pub limits: &'a ExecutionLimits,
    pub network_policy: &'a NetworkPolicy,
    pub exec: Option<ExecCallback<'a>>,
}

/// Support status of a command flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagStatus {
    /// Fully implemented with correct behavior.
    Supported,
    /// Accepted but behavior is stubbed/incomplete.
    Stubbed,
    /// Recognized but silently ignored.
    Ignored,
}

/// Metadata about a single command flag.
#[derive(Debug, Clone)]
pub struct FlagInfo {
    pub flag: &'static str,
    pub description: &'static str,
    pub status: FlagStatus,
}

/// Declarative metadata for a command, used by --help and the help builtin.
pub struct CommandMeta {
    pub name: &'static str,
    pub synopsis: &'static str,
    pub description: &'static str,
    pub options: &'static [(&'static str, &'static str)],
    pub supports_help_flag: bool,
    pub flags: &'static [FlagInfo],
}

/// Format help text from `CommandMeta` for display via `--help`.
pub fn format_help(meta: &CommandMeta) -> String {
    let mut out = format!("Usage: {}\n\n{}\n", meta.synopsis, meta.description);
    if !meta.options.is_empty() {
        out.push_str("\nOptions:\n");
        for (flag, desc) in meta.options {
            out.push_str(&format!("  {:<20} {}\n", flag, desc));
        }
    }
    if !meta.flags.is_empty() {
        out.push_str("\nFlag support:\n");
        for fi in meta.flags {
            let status_label = match fi.status {
                FlagStatus::Supported => "supported",
                FlagStatus::Stubbed => "stubbed",
                FlagStatus::Ignored => "ignored",
            };
            out.push_str(&format!(
                "  {:<20} {} [{}]\n",
                fi.flag, fi.description, status_label
            ));
        }
    }
    out
}

/// Standard error for unrecognized options, matching bash/GNU conventions.
pub fn unknown_option(cmd: &str, option: &str) -> CommandResult {
    let msg = if option.starts_with("--") {
        format!("{}: unrecognized option '{}'\n", cmd, option)
    } else {
        format!(
            "{}: invalid option -- '{}'\n",
            cmd,
            option.trim_start_matches('-')
        )
    };
    CommandResult {
        stderr: msg,
        exit_code: 2,
        ..Default::default()
    }
}

/// Trait for commands that can be registered and executed.
pub trait VirtualCommand: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult;
    fn meta(&self) -> Option<&'static CommandMeta> {
        None
    }
}

// ── Built-in command implementations ─────────────────────────────────

/// The `echo` command: prints arguments to stdout.
pub struct EchoCommand;

static ECHO_META: CommandMeta = CommandMeta {
    name: "echo",
    synopsis: "echo [-neE] [string ...]",
    description: "Write arguments to standard output.",
    options: &[
        ("-n", "do not output the trailing newline"),
        ("-e", "enable interpretation of backslash escapes"),
        ("-E", "disable interpretation of backslash escapes"),
    ],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for EchoCommand {
    fn name(&self) -> &str {
        "echo"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&ECHO_META)
    }

    fn execute(&self, args: &[String], _ctx: &CommandContext) -> CommandResult {
        let mut no_newline = false;
        let mut interpret_escapes = false;
        let mut arg_start = 0;

        for (i, arg) in args.iter().enumerate() {
            if arg.starts_with('-')
                && arg.len() > 1
                && arg[1..].chars().all(|c| matches!(c, 'n' | 'e' | 'E'))
            {
                for c in arg[1..].chars() {
                    match c {
                        'n' => no_newline = true,
                        'e' => interpret_escapes = true,
                        'E' => interpret_escapes = false,
                        _ => unreachable!(),
                    }
                }
                arg_start = i + 1;
            } else {
                break;
            }
        }

        let text = args[arg_start..].join(" ");
        let (output, suppress_newline) = if interpret_escapes {
            interpret_echo_escapes(&text)
        } else {
            (text, false)
        };

        let stdout = if no_newline || suppress_newline {
            output
        } else {
            format!("{output}\n")
        };

        CommandResult {
            stdout,
            stderr: String::new(),
            exit_code: 0,
            stdout_bytes: None,
        }
    }
}

fn interpret_echo_escapes(s: &str) -> (String, bool) {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') => {
                    chars.next();
                    result.push('\n');
                }
                Some('t') => {
                    chars.next();
                    result.push('\t');
                }
                Some('\\') => {
                    chars.next();
                    result.push('\\');
                }
                Some('a') => {
                    chars.next();
                    result.push('\x07');
                }
                Some('b') => {
                    chars.next();
                    result.push('\x08');
                }
                Some('f') => {
                    chars.next();
                    result.push('\x0C');
                }
                Some('r') => {
                    chars.next();
                    result.push('\r');
                }
                Some('v') => {
                    chars.next();
                    result.push('\x0B');
                }
                Some('c') => return (result, true),
                _ => result.push('\\'),
            }
        } else {
            result.push(c);
        }
    }
    (result, false)
}

/// The `true` command: always succeeds (exit code 0).
pub struct TrueCommand;

static TRUE_META: CommandMeta = CommandMeta {
    name: "true",
    synopsis: "true",
    description: "Do nothing, successfully.",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for TrueCommand {
    fn name(&self) -> &str {
        "true"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TRUE_META)
    }

    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult::default()
    }
}

/// The `false` command: always fails (exit code 1).
pub struct FalseCommand;

static FALSE_META: CommandMeta = CommandMeta {
    name: "false",
    synopsis: "false",
    description: "Do nothing, unsuccessfully.",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for FalseCommand {
    fn name(&self) -> &str {
        "false"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&FALSE_META)
    }

    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult {
            exit_code: 1,
            ..CommandResult::default()
        }
    }
}

/// The `cat` command: concatenate files and/or stdin.
pub struct CatCommand;

static CAT_META: CommandMeta = CommandMeta {
    name: "cat",
    synopsis: "cat [-n] [FILE ...]",
    description: "Concatenate files and print on standard output.",
    options: &[("-n, --number", "number all output lines")],
    supports_help_flag: true,
    flags: &[],
};

impl VirtualCommand for CatCommand {
    fn name(&self) -> &str {
        "cat"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&CAT_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut number_lines = false;
        let mut files: Vec<&str> = Vec::new();

        for arg in args {
            if arg == "-n" || arg == "--number" {
                number_lines = true;
            } else if arg == "-" {
                files.push("-");
            } else if arg.starts_with('-') && arg.len() > 1 {
                // Unknown flags — ignore for compatibility
            } else {
                files.push(arg);
            }
        }

        // No files specified → read from stdin
        if files.is_empty() {
            files.push("-");
        }

        let mut output = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for file in &files {
            let content = if *file == "-" || *file == "/dev/stdin" {
                ctx.stdin.to_string()
            } else if *file == "/dev/null" || *file == "/dev/zero" || *file == "/dev/full" {
                String::new()
            } else {
                let path = if file.starts_with('/') {
                    std::path::PathBuf::from(file)
                } else {
                    std::path::PathBuf::from(ctx.cwd).join(file)
                };
                match ctx.fs.read_file(&path) {
                    Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                    Err(e) => {
                        stderr.push_str(&format!("cat: {file}: {e}\n"));
                        exit_code = 1;
                        continue;
                    }
                }
            };

            if number_lines {
                let lines: Vec<&str> = content.split('\n').collect();
                let line_count = if content.ends_with('\n') && lines.last() == Some(&"") {
                    lines.len() - 1
                } else {
                    lines.len()
                };
                for (i, line) in lines.iter().take(line_count).enumerate() {
                    output.push_str(&format!("     {}\t{}", i + 1, line));
                    if i < line_count - 1 || content.ends_with('\n') {
                        output.push('\n');
                    }
                }
            } else {
                output.push_str(&content);
            }
        }

        CommandResult {
            stdout: output,
            stderr,
            exit_code,
            stdout_bytes: None,
        }
    }
}

/// The `pwd` command: print working directory.
pub struct PwdCommand;

static PWD_META: CommandMeta = CommandMeta {
    name: "pwd",
    synopsis: "pwd",
    description: "Print the current working directory.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl VirtualCommand for PwdCommand {
    fn name(&self) -> &str {
        "pwd"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&PWD_META)
    }

    fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: format!("{}\n", ctx.cwd),
            stderr: String::new(),
            exit_code: 0,
            stdout_bytes: None,
        }
    }
}

/// The `touch` command: create empty file or update mtime.
pub struct TouchCommand;

static TOUCH_META: CommandMeta = CommandMeta {
    name: "touch",
    synopsis: "touch FILE ...",
    description: "Update file access and modification times, creating files if needed.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl VirtualCommand for TouchCommand {
    fn name(&self) -> &str {
        "touch"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TOUCH_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut stderr = String::new();
        let mut exit_code = 0;

        let files: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        if files.is_empty() {
            return CommandResult {
                stdout: String::new(),
                stderr: "touch: missing file operand\n".to_string(),
                exit_code: 1,
                stdout_bytes: None,
            };
        }

        for file in files {
            if file.starts_with('-') {
                continue; // skip flags
            }
            let path = if file.starts_with('/') {
                std::path::PathBuf::from(file)
            } else {
                std::path::PathBuf::from(ctx.cwd).join(file)
            };

            if ctx.fs.exists(&path) {
                // Update mtime
                if let Err(e) = ctx.fs.utimes(&path, crate::platform::SystemTime::now()) {
                    stderr.push_str(&format!("touch: cannot touch '{}': {}\n", file, e));
                    exit_code = 1;
                }
            } else {
                // Create empty file
                if let Err(e) = ctx.fs.write_file(&path, b"") {
                    stderr.push_str(&format!("touch: cannot touch '{}': {}\n", file, e));
                    exit_code = 1;
                }
            }
        }

        CommandResult {
            stdout: String::new(),
            stderr,
            exit_code,
            stdout_bytes: None,
        }
    }
}

/// The `mkdir` command: create directories (`-p` for parents).
pub struct MkdirCommand;

static MKDIR_META: CommandMeta = CommandMeta {
    name: "mkdir",
    synopsis: "mkdir [-p] DIRECTORY ...",
    description: "Create directories.",
    options: &[("-p, --parents", "create parent directories as needed")],
    supports_help_flag: true,
    flags: &[],
};

impl VirtualCommand for MkdirCommand {
    fn name(&self) -> &str {
        "mkdir"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&MKDIR_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut parents = false;
        let mut dirs: Vec<&str> = Vec::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for arg in args {
            if arg == "-p" || arg == "--parents" {
                parents = true;
            } else if arg.starts_with('-') {
                // skip unknown flags
            } else {
                dirs.push(arg);
            }
        }

        if dirs.is_empty() {
            return CommandResult {
                stdout: String::new(),
                stderr: "mkdir: missing operand\n".to_string(),
                exit_code: 1,
                stdout_bytes: None,
            };
        }

        for dir in dirs {
            let path = if dir.starts_with('/') {
                std::path::PathBuf::from(dir)
            } else {
                std::path::PathBuf::from(ctx.cwd).join(dir)
            };

            let result = if parents {
                ctx.fs.mkdir_p(&path)
            } else {
                ctx.fs.mkdir(&path)
            };

            if let Err(e) = result {
                stderr.push_str(&format!(
                    "mkdir: cannot create directory '{}': {}\n",
                    dir, e
                ));
                exit_code = 1;
            }
        }

        CommandResult {
            stdout: String::new(),
            stderr,
            exit_code,
            stdout_bytes: None,
        }
    }
}

/// The `ls` command: list directory contents.
pub struct LsCommand;

static LS_FLAGS: &[FlagInfo] = &[
    FlagInfo {
        flag: "-a",
        description: "show hidden entries",
        status: FlagStatus::Supported,
    },
    FlagInfo {
        flag: "-l",
        description: "long listing format",
        status: FlagStatus::Supported,
    },
    FlagInfo {
        flag: "-1",
        description: "one entry per line",
        status: FlagStatus::Supported,
    },
    FlagInfo {
        flag: "-R",
        description: "recursive listing",
        status: FlagStatus::Supported,
    },
    FlagInfo {
        flag: "-t",
        description: "sort by modification time",
        status: FlagStatus::Ignored,
    },
    FlagInfo {
        flag: "-S",
        description: "sort by file size",
        status: FlagStatus::Ignored,
    },
    FlagInfo {
        flag: "-h",
        description: "human-readable sizes",
        status: FlagStatus::Ignored,
    },
];

static LS_META: CommandMeta = CommandMeta {
    name: "ls",
    synopsis: "ls [-alR1] [FILE ...]",
    description: "List directory contents.",
    options: &[
        ("-a", "do not ignore entries starting with ."),
        ("-l", "use a long listing format"),
        ("-1", "list one file per line"),
        ("-R", "list subdirectories recursively"),
    ],
    supports_help_flag: true,
    flags: LS_FLAGS,
};

impl VirtualCommand for LsCommand {
    fn name(&self) -> &str {
        "ls"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&LS_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut show_all = false;
        let mut long_format = false;
        let mut one_per_line = false;
        let mut recursive = false;
        let mut targets: Vec<&str> = Vec::new();

        for arg in args {
            if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
                for c in arg[1..].chars() {
                    match c {
                        'a' => show_all = true,
                        'l' => long_format = true,
                        '1' => one_per_line = true,
                        'R' => recursive = true,
                        _ => {}
                    }
                }
            } else {
                targets.push(arg);
            }
        }

        if targets.is_empty() {
            targets.push(".");
        }

        let opts = LsOptions {
            show_all,
            long_format,
            one_per_line,
            recursive,
        };
        let mut out = LsOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 0,
        };
        let multi_target = targets.len() > 1 || recursive;

        for (idx, target) in targets.iter().enumerate() {
            let path = if *target == "." {
                std::path::PathBuf::from(ctx.cwd)
            } else if target.starts_with('/') {
                std::path::PathBuf::from(target)
            } else {
                std::path::PathBuf::from(ctx.cwd).join(target)
            };

            if idx > 0 {
                out.stdout.push('\n');
            }

            ls_dir(ctx, &path, target, &opts, multi_target, &mut out);
        }

        CommandResult {
            stdout: out.stdout,
            stderr: out.stderr,
            exit_code: out.exit_code,
            stdout_bytes: None,
        }
    }
}

struct LsOptions {
    show_all: bool,
    long_format: bool,
    one_per_line: bool,
    recursive: bool,
}

struct LsOutput {
    stdout: String,
    stderr: String,
    exit_code: i32,
}

fn ls_dir(
    ctx: &CommandContext,
    path: &std::path::Path,
    display_name: &str,
    opts: &LsOptions,
    show_header: bool,
    out: &mut LsOutput,
) {
    let entries = match ctx.fs.readdir(path) {
        Ok(e) => e,
        Err(e) => {
            out.stderr
                .push_str(&format!("ls: cannot access '{}': {}\n", display_name, e));
            out.exit_code = 2;
            return;
        }
    };

    if show_header {
        out.stdout.push_str(&format!("{}:\n", display_name));
    }

    let mut names: Vec<(String, crate::vfs::NodeType)> = entries
        .iter()
        .filter(|e| opts.show_all || !e.name.starts_with('.'))
        .map(|e| (e.name.clone(), e.node_type))
        .collect();
    names.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));

    if opts.long_format {
        for (name, node_type) in &names {
            let child_path = path.join(name);
            let meta = ctx.fs.stat(&child_path);
            let mode = match meta {
                Ok(m) => m.mode,
                Err(_) => 0o644,
            };
            let type_char = match node_type {
                crate::vfs::NodeType::Directory => 'd',
                crate::vfs::NodeType::Symlink => 'l',
                crate::vfs::NodeType::File => '-',
            };
            out.stdout
                .push_str(&format!("{}{} {}\n", type_char, format_mode(mode), name));
        }
    } else if opts.one_per_line {
        for (name, _) in &names {
            out.stdout.push_str(name);
            out.stdout.push('\n');
        }
    } else {
        // Default: space-separated on one line
        let name_strs: Vec<&str> = names.iter().map(|(n, _)| n.as_str()).collect();
        if !name_strs.is_empty() {
            out.stdout.push_str(&name_strs.join("  "));
            out.stdout.push('\n');
        }
    }

    if opts.recursive {
        let subdirs: Vec<(String, std::path::PathBuf)> = names
            .iter()
            .filter(|(_, t)| matches!(t, crate::vfs::NodeType::Directory))
            .map(|(n, _)| (n.clone(), path.join(n)))
            .collect();

        for (name, subpath) in subdirs {
            out.stdout.push('\n');
            let sub_display = if display_name == "." {
                format!("./{}", name)
            } else {
                format!("{}/{}", display_name, name)
            };
            ls_dir(ctx, &subpath, &sub_display, opts, true, out);
        }
    }
}

fn format_mode(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    let flags = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, ch) in flags {
        s.push(if mode & bit != 0 { ch } else { '-' });
    }
    s
}

/// The `test` command: evaluate conditional expressions.
pub struct TestCommand;

static TEST_META: CommandMeta = CommandMeta {
    name: "test",
    synopsis: "test EXPRESSION",
    description: "Evaluate conditional expression.",
    options: &[
        ("-e FILE", "FILE exists"),
        ("-f FILE", "FILE exists and is a regular file"),
        ("-d FILE", "FILE exists and is a directory"),
        ("-z STRING", "the length of STRING is zero"),
        ("-n STRING", "the length of STRING is nonzero"),
        ("s1 = s2", "the strings are equal"),
        ("s1 != s2", "the strings are not equal"),
        ("n1 -eq n2", "integers are equal"),
        ("n1 -lt n2", "first integer is less than second"),
    ],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for TestCommand {
    fn name(&self) -> &str {
        "test"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TEST_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        test_cmd::evaluate_test_args(args, ctx)
    }
}

/// The `[` command: evaluate conditional expressions (requires closing `]`).
pub struct BracketCommand;

static BRACKET_META: CommandMeta = CommandMeta {
    name: "[",
    synopsis: "[ EXPRESSION ]",
    description: "Evaluate conditional expression (synonym for test).",
    options: &[],
    supports_help_flag: false,
    flags: &[],
};

impl VirtualCommand for BracketCommand {
    fn name(&self) -> &str {
        "["
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&BRACKET_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        if args.is_empty() || args.last().map(|s| s.as_str()) != Some("]") {
            return CommandResult {
                stderr: "[: missing ']'\n".to_string(),
                exit_code: 2,
                ..CommandResult::default()
            };
        }
        // Strip the closing ]
        test_cmd::evaluate_test_args(&args[..args.len() - 1], ctx)
    }
}

/// `fgrep` — alias for `grep -F`.
pub struct FgrepCommand;

static FGREP_META: CommandMeta = CommandMeta {
    name: "fgrep",
    synopsis: "fgrep [OPTIONS] PATTERN [FILE ...]",
    description: "Equivalent to grep -F (fixed-string search).",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl VirtualCommand for FgrepCommand {
    fn name(&self) -> &str {
        "fgrep"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&FGREP_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut new_args = vec!["-F".to_string()];
        new_args.extend(args.iter().cloned());
        text::GrepCommand.execute(&new_args, ctx)
    }
}

/// `egrep` — alias for `grep -E`.
pub struct EgrepCommand;

static EGREP_META: CommandMeta = CommandMeta {
    name: "egrep",
    synopsis: "egrep [OPTIONS] PATTERN [FILE ...]",
    description: "Equivalent to grep -E (extended regexp search).",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl VirtualCommand for EgrepCommand {
    fn name(&self) -> &str {
        "egrep"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&EGREP_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut new_args = vec!["-E".to_string()];
        new_args.extend(args.iter().cloned());
        text::GrepCommand.execute(&new_args, ctx)
    }
}

/// Register the default set of commands.
pub fn register_default_commands() -> HashMap<String, Box<dyn VirtualCommand>> {
    let mut commands: HashMap<String, Box<dyn VirtualCommand>> = HashMap::new();
    let defaults: Vec<Box<dyn VirtualCommand>> = vec![
        Box::new(EchoCommand),
        Box::new(TrueCommand),
        Box::new(FalseCommand),
        Box::new(CatCommand),
        Box::new(PwdCommand),
        Box::new(TouchCommand),
        Box::new(MkdirCommand),
        Box::new(LsCommand),
        Box::new(TestCommand),
        Box::new(BracketCommand),
        // Phase 10a: file operations
        Box::new(file_ops::CpCommand),
        Box::new(file_ops::MvCommand),
        Box::new(file_ops::RmCommand),
        Box::new(file_ops::TeeCommand),
        Box::new(file_ops::StatCommand),
        Box::new(file_ops::ChmodCommand),
        Box::new(file_ops::LnCommand),
        // Phase 10b: text processing
        Box::new(text::GrepCommand),
        Box::new(text::SortCommand),
        Box::new(text::UniqCommand),
        Box::new(text::CutCommand),
        Box::new(text::HeadCommand),
        Box::new(text::TailCommand),
        Box::new(text::WcCommand),
        Box::new(text::TrCommand),
        Box::new(text::RevCommand),
        Box::new(text::FoldCommand),
        Box::new(text::NlCommand),
        Box::new(text::PrintfCommand),
        Box::new(text::PasteCommand),
        Box::new(text::OdCommand),
        // M2.6: remaining text commands
        Box::new(text::TacCommand),
        Box::new(text::CommCommand),
        Box::new(text::JoinCommand),
        Box::new(text::FmtCommand),
        Box::new(text::ColumnCommand),
        Box::new(text::ExpandCommand),
        Box::new(text::UnexpandCommand),
        // Phase 10c: navigation
        Box::new(navigation::RealpathCommand),
        Box::new(navigation::BasenameCommand),
        Box::new(navigation::DirnameCommand),
        Box::new(navigation::TreeCommand),
        // Phase 10d: utilities
        Box::new(utils::ExprCommand),
        Box::new(utils::DateCommand),
        Box::new(utils::SleepCommand),
        Box::new(utils::SeqCommand),
        Box::new(utils::EnvCommand),
        Box::new(utils::PrintenvCommand),
        Box::new(utils::WhichCommand),
        Box::new(utils::Base64Command),
        Box::new(utils::Md5sumCommand),
        Box::new(utils::Sha256sumCommand),
        Box::new(utils::WhoamiCommand),
        Box::new(utils::HostnameCommand),
        Box::new(utils::UnameCommand),
        Box::new(utils::YesCommand),
        // Phase 10e: commands needing exec callback
        Box::new(exec_cmds::XargsCommand),
        Box::new(exec_cmds::FindCommand),
        // M2.5: diff
        Box::new(diff_cmd::DiffCommand),
        // M2.2: sed
        Box::new(sed::SedCommand),
        // M2.4: jq
        Box::new(jq_cmd::JqCommand),
        // M2.3: awk
        Box::new(awk::AwkCommand),
        // M7.2: core utility commands
        Box::new(utils::Sha1sumCommand),
        Box::new(utils::TimeoutCommand),
        Box::new(utils::FileCommand),
        Box::new(utils::BcCommand),
        Box::new(utils::ClearCommand),
        Box::new(FgrepCommand),
        Box::new(EgrepCommand),
        // M7.4: binary and file inspection
        Box::new(text::StringsCommand),
        // M7.5: search commands
        Box::new(text::RgCommand),
        // M7.2: file operations
        Box::new(file_ops::ReadlinkCommand),
        Box::new(file_ops::RmdirCommand),
        Box::new(file_ops::DuCommand),
        Box::new(file_ops::SplitCommand),
        // M7.3: compression and archiving
        Box::new(compression::GzipCommand),
        Box::new(compression::GunzipCommand),
        Box::new(compression::ZcatCommand),
        Box::new(compression::TarCommand),
    ];
    for cmd in defaults {
        commands.insert(cmd.name().to_string(), cmd);
    }
    // M3.2: network (feature-gated)
    #[cfg(feature = "network")]
    {
        commands.insert("curl".to_string(), Box::new(net::CurlCommand));
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::NetworkPolicy;
    use crate::vfs::InMemoryFs;
    use std::sync::Arc;

    fn test_ctx() -> (
        Arc<InMemoryFs>,
        HashMap<String, String>,
        ExecutionLimits,
        NetworkPolicy,
    ) {
        (
            Arc::new(InMemoryFs::new()),
            HashMap::new(),
            ExecutionLimits::default(),
            NetworkPolicy::default(),
        )
    }

    #[test]
    fn echo_no_args() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&[], &ctx);
        assert_eq!(result.stdout, "\n");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn echo_simple_text() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&["hello".into(), "world".into()], &ctx);
        assert_eq!(result.stdout, "hello world\n");
    }

    #[test]
    fn echo_flag_n() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&["-n".into(), "hello".into()], &ctx);
        assert_eq!(result.stdout, "hello");
    }

    #[test]
    fn echo_escape_newline() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&["-e".into(), "hello\\nworld".into()], &ctx);
        assert_eq!(result.stdout, "hello\nworld\n");
    }

    #[test]
    fn echo_escape_tab() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&["-e".into(), "a\\tb".into()], &ctx);
        assert_eq!(result.stdout, "a\tb\n");
    }

    #[test]
    fn echo_escape_stop_output() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&["-e".into(), "hello\\cworld".into()], &ctx);
        assert_eq!(result.stdout, "hello");
    }

    #[test]
    fn echo_non_flag_dash_arg() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&["-z".into(), "hello".into()], &ctx);
        assert_eq!(result.stdout, "-z hello\n");
    }

    #[test]
    fn echo_combined_flags() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        let result = EchoCommand.execute(&["-ne".into(), "hello\\nworld".into()], &ctx);
        assert_eq!(result.stdout, "hello\nworld");
    }

    #[test]
    fn true_succeeds() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        assert_eq!(TrueCommand.execute(&[], &ctx).exit_code, 0);
    }

    #[test]
    fn false_fails() {
        let (fs, env, limits, np) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            stdin_bytes: None,
            limits: &limits,
            network_policy: &np,
            exec: None,
        };
        assert_eq!(FalseCommand.execute(&[], &ctx).exit_code, 1);
    }
}
