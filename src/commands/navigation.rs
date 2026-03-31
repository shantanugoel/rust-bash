//! Navigation commands: realpath, basename, dirname, tree

use super::CommandMeta;
use crate::commands::{CommandContext, CommandResult};
use crate::vfs::NodeType;
use std::path::{Path, PathBuf};

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

// ── realpath ─────────────────────────────────────────────────────────

pub struct RealpathCommand;

static REALPATH_META: CommandMeta = CommandMeta {
    name: "realpath",
    synopsis: "realpath [PATH ...]",
    description: "Print the resolved absolute pathname.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for RealpathCommand {
    fn name(&self) -> &str {
        "realpath"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&REALPATH_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut operands = Vec::new();
        let mut opts_done = false;

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                // ignore flags
            } else {
                operands.push(arg.as_str());
            }
        }

        if operands.is_empty() {
            return CommandResult {
                stderr: "realpath: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for op in operands {
            let path = resolve_path(op, ctx.cwd);
            match ctx.fs.canonicalize(&path) {
                Ok(resolved) => {
                    stdout.push_str(&resolved.to_string_lossy());
                    stdout.push('\n');
                }
                Err(e) => {
                    stderr.push_str(&format!("realpath: {}: {}\n", op, e));
                    exit_code = 1;
                }
            }
        }

        CommandResult {
            stdout,
            stderr,
            exit_code,
            stdout_bytes: None,
        }
    }
}

// ── basename ─────────────────────────────────────────────────────────

pub struct BasenameCommand;

static BASENAME_META: CommandMeta = CommandMeta {
    name: "basename",
    synopsis: "basename NAME [SUFFIX]",
    description: "Strip directory and suffix from filenames.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for BasenameCommand {
    fn name(&self) -> &str {
        "basename"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&BASENAME_META)
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
                // ignore flags
            } else {
                operands.push(arg);
            }
        }

        if operands.is_empty() {
            return CommandResult {
                stderr: "basename: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let path = operands[0];
        let suffix = operands.get(1).copied().unwrap_or("");

        let base = if path == "/" {
            "/".to_string()
        } else {
            let trimmed = path.trim_end_matches('/');
            Path::new(trimmed)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "/".to_string())
        };

        let result = if !suffix.is_empty() && base.ends_with(suffix) && base != suffix {
            base[..base.len() - suffix.len()].to_string()
        } else {
            base
        };

        CommandResult {
            stdout: format!("{result}\n"),
            ..Default::default()
        }
    }
}

// ── dirname ──────────────────────────────────────────────────────────

pub struct DirnameCommand;

static DIRNAME_META: CommandMeta = CommandMeta {
    name: "dirname",
    synopsis: "dirname NAME ...",
    description: "Strip last component from file name.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for DirnameCommand {
    fn name(&self) -> &str {
        "dirname"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&DIRNAME_META)
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
                // ignore flags
            } else {
                operands.push(arg);
            }
        }

        if operands.is_empty() {
            return CommandResult {
                stderr: "dirname: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        for op in operands {
            let dir = if op == "/" {
                "/".to_string()
            } else {
                let trimmed = op.trim_end_matches('/');
                Path::new(trimmed)
                    .parent()
                    .map(|p| {
                        let s = p.to_string_lossy().to_string();
                        if s.is_empty() { ".".to_string() } else { s }
                    })
                    .unwrap_or_else(|| ".".to_string())
            };
            stdout.push_str(&dir);
            stdout.push('\n');
        }

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

// ── tree ─────────────────────────────────────────────────────────────

pub struct TreeCommand;

static TREE_META: CommandMeta = CommandMeta {
    name: "tree",
    synopsis: "tree [DIRECTORY]",
    description: "List contents of directories in a tree-like format.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for TreeCommand {
    fn name(&self) -> &str {
        "tree"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TREE_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut target = ".";
        let mut opts_done = false;

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                // ignore flags
                continue;
            }
            target = arg;
            break;
        }

        let path = resolve_path(target, ctx.cwd);

        if !ctx.fs.exists(&path) {
            return CommandResult {
                stderr: format!("tree: '{}': No such file or directory\n", target),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = format!("{target}\n");
        let mut dir_count: u64 = 0;
        let mut file_count: u64 = 0;

        tree_recursive(ctx, &path, "", &mut stdout, &mut dir_count, &mut file_count);

        stdout.push_str(&format!(
            "\n{} directories, {} files\n",
            dir_count, file_count
        ));

        CommandResult {
            stdout,
            ..Default::default()
        }
    }
}

fn tree_recursive(
    ctx: &CommandContext,
    path: &Path,
    prefix: &str,
    out: &mut String,
    dir_count: &mut u64,
    file_count: &mut u64,
) {
    let entries = match ctx.fs.readdir(path) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut sorted: Vec<_> = entries
        .into_iter()
        .filter(|e| !e.name.starts_with('.'))
        .collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));

    let count = sorted.len();
    for (i, entry) in sorted.iter().enumerate() {
        let is_last = i == count - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let child_prefix = if is_last { "    " } else { "│   " };

        out.push_str(&format!("{prefix}{connector}{}\n", entry.name));

        if entry.node_type == NodeType::Directory {
            *dir_count += 1;
            tree_recursive(
                ctx,
                &path.join(&entry.name),
                &format!("{prefix}{child_prefix}"),
                out,
                dir_count,
                file_count,
            );
        } else {
            *file_count += 1;
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
    use std::sync::Arc;

    fn setup() -> (
        Arc<InMemoryFs>,
        HashMap<String, String>,
        ExecutionLimits,
        NetworkPolicy,
    ) {
        let fs = Arc::new(InMemoryFs::new());
        fs.mkdir_p(Path::new("/usr/bin")).unwrap();
        fs.write_file(Path::new("/usr/bin/sort"), b"").unwrap();
        fs.mkdir_p(Path::new("/a/b")).unwrap();
        fs.write_file(Path::new("/a/b/c.txt"), b"data").unwrap();
        fs.write_file(Path::new("/a/x.txt"), b"data").unwrap();
        (
            fs,
            HashMap::new(),
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
            variables: None,
            stdin: "",
            stdin_bytes: None,
            limits,
            network_policy,
            exec: None,
        }
    }

    // ── basename tests ───────────────────────────────────────────────

    #[test]
    fn basename_simple() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = BasenameCommand.execute(&["/usr/bin/sort".into()], &c);
        assert_eq!(r.stdout, "sort\n");
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn basename_with_suffix() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = BasenameCommand.execute(&["file.txt".into(), ".txt".into()], &c);
        assert_eq!(r.stdout, "file\n");
    }

    #[test]
    fn basename_trailing_slash() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = BasenameCommand.execute(&["/usr/bin/".into()], &c);
        assert_eq!(r.stdout, "bin\n");
    }

    #[test]
    fn basename_root() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = BasenameCommand.execute(&["/".into()], &c);
        assert_eq!(r.stdout, "/\n");
    }

    #[test]
    fn basename_missing() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = BasenameCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── dirname tests ────────────────────────────────────────────────

    #[test]
    fn dirname_simple() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = DirnameCommand.execute(&["/usr/bin/sort".into()], &c);
        assert_eq!(r.stdout, "/usr/bin\n");
    }

    #[test]
    fn dirname_no_dir() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = DirnameCommand.execute(&["file.txt".into()], &c);
        assert_eq!(r.stdout, ".\n");
    }

    #[test]
    fn dirname_root() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = DirnameCommand.execute(&["/".into()], &c);
        assert_eq!(r.stdout, "/\n");
    }

    #[test]
    fn dirname_missing() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = DirnameCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── realpath tests ───────────────────────────────────────────────

    #[test]
    fn realpath_absolute() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RealpathCommand.execute(&["/a/b/c.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("/a/b/c.txt"));
    }

    #[test]
    fn realpath_missing() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RealpathCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── tree tests ───────────────────────────────────────────────────

    #[test]
    fn tree_basic() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TreeCommand.execute(&["/a".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("b"));
        assert!(r.stdout.contains("x.txt"));
        assert!(r.stdout.contains("directories"));
        assert!(r.stdout.contains("files"));
    }

    #[test]
    fn tree_nonexistent() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TreeCommand.execute(&["/nope".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn tree_nested() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TreeCommand.execute(&["/a".into()], &c);
        assert!(r.stdout.contains("c.txt"));
    }
}
