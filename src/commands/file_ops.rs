//! File operation commands: cp, mv, rm, tee, stat, chmod, ln

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

// ── cp ───────────────────────────────────────────────────────────────

pub struct CpCommand;

impl super::VirtualCommand for CpCommand {
    fn name(&self) -> &str {
        "cp"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut recursive = false;
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
                        'r' | 'R' => recursive = true,
                        _ => {}
                    }
                }
            } else {
                operands.push(arg);
            }
        }

        if operands.len() < 2 {
            return CommandResult {
                stderr: "cp: missing file operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let dest_str = operands[operands.len() - 1];
        let sources = &operands[..operands.len() - 1];
        let dest_path = resolve_path(dest_str, ctx.cwd);
        let dest_is_dir = ctx
            .fs
            .stat(&dest_path)
            .map(|m| m.node_type == NodeType::Directory)
            .unwrap_or(false);

        if sources.len() > 1 && !dest_is_dir {
            return CommandResult {
                stderr: format!("cp: target '{}' is not a directory\n", dest_str),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stderr = String::new();
        let mut exit_code = 0;

        for src_str in sources {
            let src_path = resolve_path(src_str, ctx.cwd);
            let target = if dest_is_dir {
                let name = Path::new(src_str)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| src_str.to_string());
                dest_path.join(name)
            } else {
                dest_path.clone()
            };

            match ctx.fs.stat(&src_path) {
                Ok(meta) if meta.node_type == NodeType::Directory => {
                    if !recursive {
                        stderr.push_str(&format!(
                            "cp: -r not specified; omitting directory '{}'\n",
                            src_str
                        ));
                        exit_code = 1;
                        continue;
                    }
                    if let Err(e) = copy_dir_recursive(ctx, &src_path, &target) {
                        stderr.push_str(&format!("cp: {}\n", e));
                        exit_code = 1;
                    }
                }
                Ok(_) => {
                    if let Err(e) = ctx.fs.copy(&src_path, &target) {
                        stderr.push_str(&format!("cp: cannot copy '{}': {}\n", src_str, e));
                        exit_code = 1;
                    }
                }
                Err(e) => {
                    stderr.push_str(&format!("cp: cannot stat '{}': {}\n", src_str, e));
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

fn copy_dir_recursive(ctx: &CommandContext, src: &Path, dest: &Path) -> Result<(), String> {
    ctx.fs.mkdir_p(dest).map_err(|e| e.to_string())?;

    let entries = ctx.fs.readdir(src).map_err(|e| e.to_string())?;
    for entry in entries {
        let src_child = src.join(&entry.name);
        let dest_child = dest.join(&entry.name);
        match entry.node_type {
            NodeType::Directory => {
                copy_dir_recursive(ctx, &src_child, &dest_child)?;
            }
            _ => {
                ctx.fs
                    .copy(&src_child, &dest_child)
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

// ── mv ───────────────────────────────────────────────────────────────

pub struct MvCommand;

impl super::VirtualCommand for MvCommand {
    fn name(&self) -> &str {
        "mv"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut opts_done = false;
        let mut operands: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                // ignore flags like -f, -i
            } else {
                operands.push(arg);
            }
        }

        if operands.len() < 2 {
            return CommandResult {
                stderr: "mv: missing file operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let dest_str = operands[operands.len() - 1];
        let sources = &operands[..operands.len() - 1];
        let dest_path = resolve_path(dest_str, ctx.cwd);
        let dest_is_dir = ctx
            .fs
            .stat(&dest_path)
            .map(|m| m.node_type == NodeType::Directory)
            .unwrap_or(false);

        if sources.len() > 1 && !dest_is_dir {
            return CommandResult {
                stderr: format!("mv: target '{}' is not a directory\n", dest_str),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stderr = String::new();
        let mut exit_code = 0;

        for src_str in sources {
            let src_path = resolve_path(src_str, ctx.cwd);
            let target = if dest_is_dir {
                let name = Path::new(src_str)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| src_str.to_string());
                dest_path.join(name)
            } else {
                dest_path.clone()
            };

            if let Err(e) = ctx.fs.rename(&src_path, &target) {
                stderr.push_str(&format!("mv: cannot move '{}': {}\n", src_str, e));
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

// ── rm ───────────────────────────────────────────────────────────────

pub struct RmCommand;

impl super::VirtualCommand for RmCommand {
    fn name(&self) -> &str {
        "rm"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut recursive = false;
        let mut force = false;
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
                        'r' | 'R' => recursive = true,
                        'f' => force = true,
                        _ => {}
                    }
                }
            } else {
                operands.push(arg);
            }
        }

        if operands.is_empty() {
            if force {
                return CommandResult::default();
            }
            return CommandResult {
                stderr: "rm: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stderr = String::new();
        let mut exit_code = 0;

        for op in operands {
            let path = resolve_path(op, ctx.cwd);

            match ctx.fs.stat(&path) {
                Ok(meta) if meta.node_type == NodeType::Directory => {
                    if !recursive {
                        stderr.push_str(&format!("rm: cannot remove '{}': Is a directory\n", op));
                        exit_code = 1;
                        continue;
                    }
                    if let Err(e) = ctx.fs.remove_dir_all(&path) {
                        stderr.push_str(&format!("rm: cannot remove '{}': {}\n", op, e));
                        exit_code = 1;
                    }
                }
                Ok(_) => {
                    if let Err(e) = ctx.fs.remove_file(&path) {
                        stderr.push_str(&format!("rm: cannot remove '{}': {}\n", op, e));
                        exit_code = 1;
                    }
                }
                Err(_) => {
                    if !force {
                        stderr.push_str(&format!(
                            "rm: cannot remove '{}': No such file or directory\n",
                            op
                        ));
                        exit_code = 1;
                    }
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

// ── tee ──────────────────────────────────────────────────────────────

pub struct TeeCommand;

impl super::VirtualCommand for TeeCommand {
    fn name(&self) -> &str {
        "tee"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut append = false;
        let mut opts_done = false;
        let mut files: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    if c == 'a' {
                        append = true;
                    }
                }
            } else {
                files.push(arg);
            }
        }

        let data = ctx.stdin;
        let mut stderr = String::new();
        let mut exit_code = 0;

        for file in &files {
            let path = resolve_path(file, ctx.cwd);
            let result = if append {
                ctx.fs.append_file(&path, data.as_bytes())
            } else {
                ctx.fs.write_file(&path, data.as_bytes())
            };
            if let Err(e) = result {
                stderr.push_str(&format!("tee: {}: {}\n", file, e));
                exit_code = 1;
            }
        }

        CommandResult {
            stdout: data.to_string(),
            stderr,
            exit_code,
        }
    }
}

// ── stat ─────────────────────────────────────────────────────────────

pub struct StatCommand;

impl super::VirtualCommand for StatCommand {
    fn name(&self) -> &str {
        "stat"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut opts_done = false;
        let mut operands: Vec<&str> = Vec::new();

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
                stderr: "stat: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for op in operands {
            let path = resolve_path(op, ctx.cwd);
            match ctx.fs.stat(&path) {
                Ok(meta) => {
                    let type_str = match meta.node_type {
                        NodeType::File => "regular file",
                        NodeType::Directory => "directory",
                        NodeType::Symlink => "symbolic link",
                    };
                    stdout.push_str(&format!("  File: {}\n", op));
                    stdout.push_str(&format!("  Size: {}\tType: {}\n", meta.size, type_str));
                    stdout.push_str(&format!("  Mode: ({:04o}/-)\n", meta.mode));
                }
                Err(e) => {
                    stderr.push_str(&format!("stat: cannot stat '{}': {}\n", op, e));
                    exit_code = 1;
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

// ── chmod ────────────────────────────────────────────────────────────

pub struct ChmodCommand;

impl super::VirtualCommand for ChmodCommand {
    fn name(&self) -> &str {
        "chmod"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut opts_done = false;
        let mut operands: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                // ignore flags like -R
            } else {
                operands.push(arg);
            }
        }

        if operands.len() < 2 {
            return CommandResult {
                stderr: "chmod: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mode_str = operands[0];
        let mode = match u32::from_str_radix(mode_str, 8) {
            Ok(m) => m,
            Err(_) => {
                return CommandResult {
                    stderr: format!("chmod: invalid mode: '{}'\n", mode_str),
                    exit_code: 1,
                    ..Default::default()
                };
            }
        };

        let mut stderr = String::new();
        let mut exit_code = 0;

        for file in &operands[1..] {
            let path = resolve_path(file, ctx.cwd);
            if let Err(e) = ctx.fs.chmod(&path, mode) {
                stderr.push_str(&format!("chmod: cannot change mode of '{}': {}\n", file, e));
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

// ── ln ───────────────────────────────────────────────────────────────

pub struct LnCommand;

impl super::VirtualCommand for LnCommand {
    fn name(&self) -> &str {
        "ln"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut symbolic = false;
        let mut opts_done = false;
        let mut operands: Vec<&str> = Vec::new();

        for arg in args {
            if !opts_done && arg == "--" {
                opts_done = true;
                continue;
            }
            if !opts_done && arg.starts_with('-') && arg.len() > 1 {
                for c in arg[1..].chars() {
                    if c == 's' {
                        symbolic = true;
                    }
                }
            } else {
                operands.push(arg);
            }
        }

        if operands.len() < 2 {
            return CommandResult {
                stderr: "ln: missing file operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let target_str = operands[0];
        let link_str = operands[1];
        let target_path = resolve_path(target_str, ctx.cwd);
        let link_path = resolve_path(link_str, ctx.cwd);

        let result = if symbolic {
            ctx.fs.symlink(&target_path, &link_path)
        } else {
            ctx.fs.hardlink(&target_path, &link_path)
        };

        match result {
            Ok(()) => CommandResult::default(),
            Err(e) => CommandResult {
                stderr: format!("ln: failed to create link '{}': {}\n", link_str, e),
                exit_code: 1,
                ..Default::default()
            },
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
    use std::sync::Arc;

    fn setup() -> (Arc<InMemoryFs>, HashMap<String, String>, ExecutionLimits) {
        let fs = Arc::new(InMemoryFs::new());
        fs.write_file(Path::new("/file1.txt"), b"hello\n").unwrap();
        fs.write_file(Path::new("/file2.txt"), b"world\n").unwrap();
        fs.mkdir_p(Path::new("/dir1")).unwrap();
        fs.write_file(Path::new("/dir1/a.txt"), b"aaa\n").unwrap();
        (fs, HashMap::new(), ExecutionLimits::default())
    }

    fn ctx<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin: "",
            limits,
            exec: None,
        }
    }

    // ── cp tests ─────────────────────────────────────────────────────

    #[test]
    fn cp_basic_file() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CpCommand.execute(&["file1.txt".into(), "copy.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/copy.txt")).unwrap(), b"hello\n");
    }

    #[test]
    fn cp_into_directory() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CpCommand.execute(&["file1.txt".into(), "dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(
            fs.read_file(Path::new("/dir1/file1.txt")).unwrap(),
            b"hello\n"
        );
    }

    #[test]
    fn cp_recursive() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CpCommand.execute(&["-r".into(), "dir1".into(), "dir2".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/dir2/a.txt")).unwrap(), b"aaa\n");
    }

    #[test]
    fn cp_dir_without_r_fails() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CpCommand.execute(&["dir1".into(), "dir2".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("omitting directory"));
    }

    #[test]
    fn cp_missing_operand() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CpCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn cp_nonexistent_source() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CpCommand.execute(&["nope.txt".into(), "out.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("cannot stat"));
    }

    // ── mv tests ─────────────────────────────────────────────────────

    #[test]
    fn mv_basic() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = MvCommand.execute(&["file1.txt".into(), "moved.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(fs.read_file(Path::new("/moved.txt")).is_ok());
        assert!(!fs.exists(Path::new("/file1.txt")));
    }

    #[test]
    fn mv_into_directory() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = MvCommand.execute(&["file1.txt".into(), "dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(fs.read_file(Path::new("/dir1/file1.txt")).is_ok());
    }

    #[test]
    fn mv_missing_operand() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = MvCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── rm tests ─────────────────────────────────────────────────────

    #[test]
    fn rm_file() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = RmCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(!fs.exists(Path::new("/file1.txt")));
    }

    #[test]
    fn rm_force_nonexistent() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = RmCommand.execute(&["-f".into(), "nope.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn rm_dir_without_r_fails() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = RmCommand.execute(&["dir1".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("Is a directory"));
    }

    #[test]
    fn rm_recursive_dir() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = RmCommand.execute(&["-rf".into(), "dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(!fs.exists(Path::new("/dir1")));
    }

    #[test]
    fn rm_no_args() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = RmCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn rm_force_no_args() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = RmCommand.execute(&["-f".into()], &c);
        assert_eq!(r.exit_code, 0);
    }

    // ── tee tests ────────────────────────────────────────────────────

    #[test]
    fn tee_write_to_file_and_stdout() {
        let (fs, env, limits) = setup();
        let c = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "piped data",
            limits: &limits,
            exec: None,
        };
        let r = TeeCommand.execute(&["output.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(r.stdout, "piped data");
        assert_eq!(
            fs.read_file(Path::new("/output.txt")).unwrap(),
            b"piped data"
        );
    }

    #[test]
    fn tee_append() {
        let (fs, env, limits) = setup();
        let c = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "more",
            limits: &limits,
            exec: None,
        };
        let r = TeeCommand.execute(&["-a".into(), "file1.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(
            fs.read_file(Path::new("/file1.txt")).unwrap(),
            b"hello\nmore"
        );
    }

    // ── stat tests ───────────────────────────────────────────────────

    #[test]
    fn stat_file() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = StatCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("file1.txt"));
        assert!(r.stdout.contains("regular file"));
    }

    #[test]
    fn stat_directory() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = StatCommand.execute(&["dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("directory"));
    }

    #[test]
    fn stat_nonexistent() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = StatCommand.execute(&["nope".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── chmod tests ──────────────────────────────────────────────────

    #[test]
    fn chmod_basic() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = ChmodCommand.execute(&["755".into(), "file1.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        let meta = fs.stat(Path::new("/file1.txt")).unwrap();
        assert_eq!(meta.mode, 0o755);
    }

    #[test]
    fn chmod_invalid_mode() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = ChmodCommand.execute(&["xyz".into(), "file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("invalid mode"));
    }

    #[test]
    fn chmod_missing_operand() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = ChmodCommand.execute(&["755".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── ln tests ─────────────────────────────────────────────────────

    #[test]
    fn ln_symbolic() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = LnCommand.execute(&["-s".into(), "file1.txt".into(), "link.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        let meta = fs.lstat(Path::new("/link.txt")).unwrap();
        assert_eq!(meta.node_type, NodeType::Symlink);
    }

    #[test]
    fn ln_hard() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = LnCommand.execute(&["file1.txt".into(), "hard.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/hard.txt")).unwrap(), b"hello\n");
    }

    #[test]
    fn ln_missing_operand() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = LnCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── double-dash tests ────────────────────────────────────────────

    #[test]
    fn cp_double_dash() {
        let (fs, env, limits) = setup();
        let c = ctx(&*fs, &env, &limits);
        let r = CpCommand.execute(&["--".into(), "file1.txt".into(), "dd.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/dd.txt")).unwrap(), b"hello\n");
    }
}
