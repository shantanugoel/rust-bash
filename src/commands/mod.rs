//! Command trait and built-in command implementations.

pub(crate) mod exec_cmds;
pub(crate) mod file_ops;
pub(crate) mod navigation;
pub(crate) mod test_cmd;
pub(crate) mod text;
pub(crate) mod utils;

use crate::error::RustBashError;
use crate::interpreter::ExecutionLimits;
use crate::vfs::VirtualFs;
use std::collections::HashMap;

/// Result of executing a command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Callback type for sub-command execution (e.g. `xargs`, `find -exec`).
pub type ExecCallback<'a> = &'a dyn Fn(&str) -> Result<CommandResult, RustBashError>;

/// Context passed to command execution.
pub struct CommandContext<'a> {
    pub fs: &'a dyn VirtualFs,
    pub cwd: &'a str,
    pub env: &'a HashMap<String, String>,
    pub stdin: &'a str,
    pub limits: &'a ExecutionLimits,
    pub exec: Option<ExecCallback<'a>>,
}

/// Trait for commands that can be registered and executed.
pub trait VirtualCommand: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult;
}

// ── Built-in command implementations ─────────────────────────────────

/// The `echo` command: prints arguments to stdout.
pub struct EchoCommand;

impl VirtualCommand for EchoCommand {
    fn name(&self) -> &str {
        "echo"
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

impl VirtualCommand for TrueCommand {
    fn name(&self) -> &str {
        "true"
    }

    fn execute(&self, _args: &[String], _ctx: &CommandContext) -> CommandResult {
        CommandResult::default()
    }
}

/// The `false` command: always fails (exit code 1).
pub struct FalseCommand;

impl VirtualCommand for FalseCommand {
    fn name(&self) -> &str {
        "false"
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

impl VirtualCommand for CatCommand {
    fn name(&self) -> &str {
        "cat"
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
            let content = if *file == "-" {
                ctx.stdin.to_string()
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
        }
    }
}

/// The `pwd` command: print working directory.
pub struct PwdCommand;

impl VirtualCommand for PwdCommand {
    fn name(&self) -> &str {
        "pwd"
    }

    fn execute(&self, _args: &[String], ctx: &CommandContext) -> CommandResult {
        CommandResult {
            stdout: format!("{}\n", ctx.cwd),
            stderr: String::new(),
            exit_code: 0,
        }
    }
}

/// The `touch` command: create empty file or update mtime.
pub struct TouchCommand;

impl VirtualCommand for TouchCommand {
    fn name(&self) -> &str {
        "touch"
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
                if let Err(e) = ctx.fs.utimes(&path, std::time::SystemTime::now()) {
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
        }
    }
}

/// The `mkdir` command: create directories (`-p` for parents).
pub struct MkdirCommand;

impl VirtualCommand for MkdirCommand {
    fn name(&self) -> &str {
        "mkdir"
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
        }
    }
}

/// The `ls` command: list directory contents.
pub struct LsCommand;

impl VirtualCommand for LsCommand {
    fn name(&self) -> &str {
        "ls"
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

impl VirtualCommand for TestCommand {
    fn name(&self) -> &str {
        "test"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        test_cmd::evaluate_test_args(args, ctx)
    }
}

/// The `[` command: evaluate conditional expressions (requires closing `]`).
pub struct BracketCommand;

impl VirtualCommand for BracketCommand {
    fn name(&self) -> &str {
        "["
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
    ];
    for cmd in defaults {
        commands.insert(cmd.name().to_string(), cmd);
    }
    commands
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vfs::InMemoryFs;
    use std::sync::Arc;

    fn test_ctx() -> (Arc<InMemoryFs>, HashMap<String, String>, ExecutionLimits) {
        (
            Arc::new(InMemoryFs::new()),
            HashMap::new(),
            ExecutionLimits::default(),
        )
    }

    #[test]
    fn echo_no_args() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&[], &ctx);
        assert_eq!(result.stdout, "\n");
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn echo_simple_text() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&["hello".into(), "world".into()], &ctx);
        assert_eq!(result.stdout, "hello world\n");
    }

    #[test]
    fn echo_flag_n() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&["-n".into(), "hello".into()], &ctx);
        assert_eq!(result.stdout, "hello");
    }

    #[test]
    fn echo_escape_newline() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&["-e".into(), "hello\\nworld".into()], &ctx);
        assert_eq!(result.stdout, "hello\nworld\n");
    }

    #[test]
    fn echo_escape_tab() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&["-e".into(), "a\\tb".into()], &ctx);
        assert_eq!(result.stdout, "a\tb\n");
    }

    #[test]
    fn echo_escape_stop_output() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&["-e".into(), "hello\\cworld".into()], &ctx);
        assert_eq!(result.stdout, "hello");
    }

    #[test]
    fn echo_non_flag_dash_arg() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&["-z".into(), "hello".into()], &ctx);
        assert_eq!(result.stdout, "-z hello\n");
    }

    #[test]
    fn echo_combined_flags() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        let result = EchoCommand.execute(&["-ne".into(), "hello\\nworld".into()], &ctx);
        assert_eq!(result.stdout, "hello\nworld");
    }

    #[test]
    fn true_succeeds() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        assert_eq!(TrueCommand.execute(&[], &ctx).exit_code, 0);
    }

    #[test]
    fn false_fails() {
        let (fs, env, limits) = test_ctx();
        let ctx = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "",
            limits: &limits,
            exec: None,
        };
        assert_eq!(FalseCommand.execute(&[], &ctx).exit_code, 1);
    }

    #[test]
    fn register_default_commands_includes_expected() {
        let cmds = register_default_commands();
        assert!(cmds.contains_key("echo"));
        assert!(cmds.contains_key("true"));
        assert!(cmds.contains_key("false"));
    }
}
