//! File operation commands: cp, mv, rm, tee, stat, chmod, ln

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

// ── cp ───────────────────────────────────────────────────────────────

pub struct CpCommand;

static CP_META: CommandMeta = CommandMeta {
    name: "cp",
    synopsis: "cp [-rR] SOURCE... DEST",
    description: "Copy files and directories.",
    options: &[("-r, -R", "copy directories recursively")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for CpCommand {
    fn name(&self) -> &str {
        "cp"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&CP_META)
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

static MV_META: CommandMeta = CommandMeta {
    name: "mv",
    synopsis: "mv SOURCE... DEST",
    description: "Move (rename) files and directories.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for MvCommand {
    fn name(&self) -> &str {
        "mv"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&MV_META)
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

static RM_META: CommandMeta = CommandMeta {
    name: "rm",
    synopsis: "rm [-rf] FILE...",
    description: "Remove files or directories.",
    options: &[
        (
            "-r, -R",
            "remove directories and their contents recursively",
        ),
        ("-f", "ignore nonexistent files, never prompt"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for RmCommand {
    fn name(&self) -> &str {
        "rm"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&RM_META)
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

static TEE_META: CommandMeta = CommandMeta {
    name: "tee",
    synopsis: "tee [-a] [FILE ...]",
    description: "Read from stdin and write to stdout and files.",
    options: &[("-a", "append to the given files, do not overwrite")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for TeeCommand {
    fn name(&self) -> &str {
        "tee"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TEE_META)
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

static STAT_META: CommandMeta = CommandMeta {
    name: "stat",
    synopsis: "stat FILE...",
    description: "Display file status.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for StatCommand {
    fn name(&self) -> &str {
        "stat"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&STAT_META)
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

static CHMOD_META: CommandMeta = CommandMeta {
    name: "chmod",
    synopsis: "chmod MODE FILE...",
    description: "Change file mode bits.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for ChmodCommand {
    fn name(&self) -> &str {
        "chmod"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&CHMOD_META)
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

static LN_META: CommandMeta = CommandMeta {
    name: "ln",
    synopsis: "ln [-s] TARGET LINK_NAME",
    description: "Make links between files.",
    options: &[("-s", "make symbolic links instead of hard links")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for LnCommand {
    fn name(&self) -> &str {
        "ln"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&LN_META)
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

// ── readlink ─────────────────────────────────────────────────────────

pub struct ReadlinkCommand;

static READLINK_META: CommandMeta = CommandMeta {
    name: "readlink",
    synopsis: "readlink [-f|-e|-m] FILE",
    description: "Print resolved symbolic links or canonical file names.",
    options: &[
        (
            "-f",
            "canonicalize by following every symlink; all components must exist",
        ),
        (
            "-e",
            "like -f, but error if the final component does not exist",
        ),
        ("-m", "canonicalize without existence requirement"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for ReadlinkCommand {
    fn name(&self) -> &str {
        "readlink"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&READLINK_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut mode = 'r'; // default: read one symlink level
        let mut files: Vec<&str> = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-f" => mode = 'f',
                "-e" => mode = 'e',
                "-m" => mode = 'm',
                _ if arg.starts_with('-') && arg.len() > 1 => {
                    // absorb unknown flags
                }
                _ => files.push(arg),
            }
        }

        if files.is_empty() {
            return CommandResult {
                stderr: "readlink: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        for file in &files {
            let path = resolve_path(file, ctx.cwd);
            match mode {
                'f' | 'e' => match ctx.fs.canonicalize(&path) {
                    Ok(resolved) => {
                        if mode == 'e' && !ctx.fs.exists(&resolved) {
                            stderr.push_str(&format!(
                                "readlink: {}: No such file or directory\n",
                                file
                            ));
                            exit_code = 1;
                        } else {
                            stdout.push_str(&resolved.to_string_lossy());
                            stdout.push('\n');
                        }
                    }
                    Err(e) => {
                        stderr.push_str(&format!("readlink: {}: {}\n", file, e));
                        exit_code = 1;
                    }
                },
                'm' => {
                    // Canonicalize without existence check — just normalize the path
                    let normalized = normalize_path(&path);
                    stdout.push_str(&normalized.to_string_lossy());
                    stdout.push('\n');
                }
                _ => {
                    // Default: just read the symlink target
                    match ctx.fs.readlink(&path) {
                        Ok(target) => {
                            stdout.push_str(&target.to_string_lossy());
                            stdout.push('\n');
                        }
                        Err(e) => {
                            stderr.push_str(&format!("readlink: {}: {}\n", file, e));
                            exit_code = 1;
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

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();
    for comp in path.components() {
        match comp {
            std::path::Component::RootDir => {
                components.clear();
                components.push("/".to_string());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if components.len() > 1 {
                    components.pop();
                }
            }
            std::path::Component::Normal(s) => {
                components.push(s.to_string_lossy().to_string());
            }
            _ => {}
        }
    }
    if components.len() == 1 && components[0] == "/" {
        return PathBuf::from("/");
    }
    let mut result = String::new();
    for (i, c) in components.iter().enumerate() {
        if i == 0 && c == "/" {
            result.push('/');
        } else if i == 1 && components[0] == "/" {
            result.push_str(c);
        } else {
            result.push('/');
            result.push_str(c);
        }
    }
    PathBuf::from(result)
}

// ── rmdir ───────────────────────────────────────────────────────────

pub struct RmdirCommand;

static RMDIR_META: CommandMeta = CommandMeta {
    name: "rmdir",
    synopsis: "rmdir [-p] DIRECTORY...",
    description: "Remove empty directories.",
    options: &[("-p", "remove DIRECTORY and its ancestors")],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for RmdirCommand {
    fn name(&self) -> &str {
        "rmdir"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&RMDIR_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut parents = false;
        let mut dirs: Vec<&str> = Vec::new();

        for arg in args {
            match arg.as_str() {
                "-p" | "--parents" => parents = true,
                _ if arg.starts_with('-') && arg.len() > 1 => {}
                _ => dirs.push(arg),
            }
        }

        if dirs.is_empty() {
            return CommandResult {
                stderr: "rmdir: missing operand\n".into(),
                exit_code: 1,
                ..Default::default()
            };
        }

        let mut stderr = String::new();
        let mut exit_code = 0;

        for dir in &dirs {
            let path = resolve_path(dir, ctx.cwd);

            // Check if directory is empty
            match ctx.fs.readdir(&path) {
                Ok(entries) => {
                    if !entries.is_empty() {
                        stderr.push_str(&format!(
                            "rmdir: failed to remove '{}': Directory not empty\n",
                            dir
                        ));
                        exit_code = 1;
                        continue;
                    }
                }
                Err(e) => {
                    stderr.push_str(&format!("rmdir: failed to remove '{}': {}\n", dir, e));
                    exit_code = 1;
                    continue;
                }
            }

            if let Err(e) = ctx.fs.remove_dir(&path) {
                stderr.push_str(&format!("rmdir: failed to remove '{}': {}\n", dir, e));
                exit_code = 1;
                continue;
            }

            if parents {
                let mut current = path.parent().map(|p| p.to_path_buf());
                while let Some(parent) = current {
                    if parent == Path::new("/") || parent.as_os_str().is_empty() {
                        break;
                    }
                    match ctx.fs.readdir(&parent) {
                        Ok(entries) if entries.is_empty() => {
                            if ctx.fs.remove_dir(&parent).is_err() {
                                break;
                            }
                        }
                        _ => break,
                    }
                    current = parent.parent().map(|p| p.to_path_buf());
                }
            }
        }

        CommandResult {
            stderr,
            exit_code,
            ..Default::default()
        }
    }
}

// ── du ──────────────────────────────────────────────────────────────

pub struct DuCommand;

static DU_META: CommandMeta = CommandMeta {
    name: "du",
    synopsis: "du [-shad N] [FILE...]",
    description: "Estimate file space usage.",
    options: &[
        ("-s", "display only a total for each argument"),
        ("-h", "print sizes in human readable format"),
        ("-a", "write counts for all files, not just directories"),
        (
            "-d N",
            "print total for a directory only if it is N or fewer levels below",
        ),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for DuCommand {
    fn name(&self) -> &str {
        "du"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&DU_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut summary = false;
        let mut human = false;
        let mut all_files = false;
        let mut max_depth: Option<usize> = None;
        let mut targets: Vec<&str> = Vec::new();
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            if arg == "-s" {
                summary = true;
            } else if arg == "-h" {
                human = true;
            } else if arg == "-a" {
                all_files = true;
            } else if arg == "-d" {
                i += 1;
                if i < args.len() {
                    max_depth = args[i].parse().ok();
                }
            } else if let Some(val) = arg.strip_prefix("-d") {
                max_depth = val.parse().ok();
            } else if arg.starts_with('-') && arg.len() > 1 {
                // combined flags
                for c in arg[1..].chars() {
                    match c {
                        's' => summary = true,
                        'h' => human = true,
                        'a' => all_files = true,
                        _ => {}
                    }
                }
            } else {
                targets.push(arg);
            }
            i += 1;
        }

        if targets.is_empty() {
            targets.push(".");
        }

        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit_code = 0;

        let opts = DuOpts {
            max_depth,
            summary,
            human,
            all_files,
        };

        for target in &targets {
            let path = resolve_path(target, ctx.cwd);
            match du_walk(ctx, &path, target, 0, &opts) {
                Ok((size, output)) => {
                    if opts.summary {
                        stdout.push_str(&format!(
                            "{}\t{}\n",
                            format_du_size(size, opts.human),
                            target
                        ));
                    } else {
                        stdout.push_str(&output);
                    }
                }
                Err(e) => {
                    stderr.push_str(&format!("du: cannot access '{}': {}\n", target, e));
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

struct DuOpts {
    max_depth: Option<usize>,
    summary: bool,
    human: bool,
    all_files: bool,
}

fn du_walk(
    ctx: &CommandContext,
    path: &Path,
    display: &str,
    depth: usize,
    opts: &DuOpts,
) -> Result<(u64, String), String> {
    let meta = ctx.fs.stat(path).map_err(|e| e.to_string())?;

    if meta.node_type != NodeType::Directory {
        return Ok((meta.size, String::new()));
    }

    let entries = ctx.fs.readdir(path).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    let mut output = String::new();

    for entry in &entries {
        let child_path = path.join(&entry.name);
        let child_display = if display == "." {
            format!("./{}", entry.name)
        } else {
            format!("{}/{}", display, entry.name)
        };

        match entry.node_type {
            NodeType::Directory => {
                let (child_size, child_output) =
                    du_walk(ctx, &child_path, &child_display, depth + 1, opts)?;
                total += child_size;
                if !opts.summary {
                    output.push_str(&child_output);
                }
            }
            _ => {
                if let Ok(m) = ctx.fs.stat(&child_path) {
                    total += m.size;
                    if opts.all_files
                        && !opts.summary
                        && (opts.max_depth.is_none() || depth < opts.max_depth.unwrap())
                    {
                        output.push_str(&format!(
                            "{}\t{}\n",
                            format_du_size(m.size, opts.human),
                            child_display
                        ));
                    }
                }
            }
        }
    }

    if !opts.summary && (opts.max_depth.is_none() || depth <= opts.max_depth.unwrap()) {
        output.push_str(&format!(
            "{}\t{}\n",
            format_du_size(total, opts.human),
            display
        ));
    }

    Ok((total, output))
}

fn format_du_size(size: u64, human: bool) -> String {
    if !human {
        // du reports in 1024-byte blocks by default
        return size.div_ceil(1024).to_string();
    }
    if size < 1024 {
        format!("{}B", size)
    } else if size < 1024 * 1024 {
        format!("{:.1}K", size as f64 / 1024.0)
    } else if size < 1024 * 1024 * 1024 {
        format!("{:.1}M", size as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1}G", size as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

// ── split ───────────────────────────────────────────────────────────

pub struct SplitCommand;

static SPLIT_META: CommandMeta = CommandMeta {
    name: "split",
    synopsis: "split [-l LINES] [-b BYTES] [-a SUFFIX_LEN] [FILE [PREFIX]]",
    description: "Split a file into pieces.",
    options: &[
        ("-l LINES", "put LINES lines per output file (default 1000)"),
        ("-b BYTES", "put BYTES bytes per output file"),
        ("-a N", "generate suffixes of length N (default 2)"),
    ],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for SplitCommand {
    fn name(&self) -> &str {
        "split"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&SPLIT_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut lines_per_file: Option<usize> = None;
        let mut bytes_per_file: Option<usize> = None;
        let mut suffix_len: usize = 2;
        let mut input_file: Option<&str> = None;
        let mut prefix = "x";
        let mut i = 0;

        while i < args.len() {
            let arg = &args[i];
            if arg == "-l" {
                i += 1;
                if i < args.len() {
                    lines_per_file = args[i].parse().ok();
                }
            } else if arg == "-b" {
                i += 1;
                if i < args.len() {
                    bytes_per_file = args[i].parse().ok();
                }
            } else if arg == "-a" {
                i += 1;
                if i < args.len() {
                    suffix_len = args[i].parse().unwrap_or(2);
                }
            } else if arg.starts_with('-') && arg.len() > 1 {
                // Try combined: -l5, etc.
                if let Some(v) = arg.strip_prefix("-l") {
                    lines_per_file = v.parse().ok();
                } else if let Some(v) = arg.strip_prefix("-b") {
                    bytes_per_file = v.parse().ok();
                } else if let Some(v) = arg.strip_prefix("-a") {
                    suffix_len = v.parse().unwrap_or(2);
                }
            } else if arg == "--" {
                // skip
            } else if input_file.is_none() {
                input_file = Some(arg.as_str());
            } else {
                prefix = arg.as_str();
            }
            i += 1;
        }

        let data = match input_file {
            Some("-") | None => ctx.stdin.as_bytes().to_vec(),
            Some(f) => {
                let path = resolve_path(f, ctx.cwd);
                match ctx.fs.read_file(&path) {
                    Ok(d) => d,
                    Err(e) => {
                        return CommandResult {
                            stderr: format!("split: {}: {}\n", f, e),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                }
            }
        };

        let chunks: Vec<Vec<u8>> = if let Some(n) = bytes_per_file {
            if n == 0 {
                return CommandResult {
                    stderr: "split: invalid number of bytes: 0\n".into(),
                    exit_code: 1,
                    ..Default::default()
                };
            }
            data.chunks(n).map(|c| c.to_vec()).collect()
        } else {
            let n = lines_per_file.unwrap_or(1000);
            if n == 0 {
                return CommandResult {
                    stderr: "split: invalid number of lines: 0\n".into(),
                    exit_code: 1,
                    ..Default::default()
                };
            }
            let content = String::from_utf8_lossy(&data);
            let lines: Vec<&str> = content.lines().collect();
            lines
                .chunks(n)
                .map(|chunk| {
                    let mut s = chunk.join("\n");
                    if !s.is_empty() {
                        s.push('\n');
                    }
                    s.into_bytes()
                })
                .collect()
        };

        let mut stderr = String::new();
        let mut exit_code = 0;

        for (idx, chunk) in chunks.iter().enumerate() {
            let suffix = match split_suffix(idx, suffix_len) {
                Some(s) => s,
                None => {
                    return CommandResult {
                        stderr: "split: output file suffixes exhausted\n".into(),
                        exit_code: 1,
                        ..Default::default()
                    };
                }
            };
            let filename = format!("{}{}", prefix, suffix);
            let path = resolve_path(&filename, ctx.cwd);
            if let Err(e) = ctx.fs.write_file(&path, chunk) {
                stderr.push_str(&format!("split: {}: {}\n", filename, e));
                exit_code = 1;
            }
        }

        CommandResult {
            stderr,
            exit_code,
            ..Default::default()
        }
    }
}

fn split_suffix(idx: usize, len: usize) -> Option<String> {
    let max = 26usize.pow(len as u32);
    if idx >= max {
        return None;
    }
    let mut suffix = String::with_capacity(len);
    let mut remaining = idx;
    for i in (0..len).rev() {
        let divisor = 26usize.pow(i as u32);
        let ch = (remaining / divisor) as u8 + b'a';
        suffix.push(ch as char);
        remaining %= divisor;
    }
    Some(suffix)
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
        fs.write_file(Path::new("/file1.txt"), b"hello\n").unwrap();
        fs.write_file(Path::new("/file2.txt"), b"world\n").unwrap();
        fs.mkdir_p(Path::new("/dir1")).unwrap();
        fs.write_file(Path::new("/dir1/a.txt"), b"aaa\n").unwrap();
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
            stdin: "",
            limits,
            network_policy,
            exec: None,
        }
    }

    // ── cp tests ─────────────────────────────────────────────────────

    #[test]
    fn cp_basic_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CpCommand.execute(&["file1.txt".into(), "copy.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/copy.txt")).unwrap(), b"hello\n");
    }

    #[test]
    fn cp_into_directory() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CpCommand.execute(&["file1.txt".into(), "dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(
            fs.read_file(Path::new("/dir1/file1.txt")).unwrap(),
            b"hello\n"
        );
    }

    #[test]
    fn cp_recursive() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CpCommand.execute(&["-r".into(), "dir1".into(), "dir2".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/dir2/a.txt")).unwrap(), b"aaa\n");
    }

    #[test]
    fn cp_dir_without_r_fails() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CpCommand.execute(&["dir1".into(), "dir2".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("omitting directory"));
    }

    #[test]
    fn cp_missing_operand() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CpCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn cp_nonexistent_source() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CpCommand.execute(&["nope.txt".into(), "out.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("cannot stat"));
    }

    // ── mv tests ─────────────────────────────────────────────────────

    #[test]
    fn mv_basic() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = MvCommand.execute(&["file1.txt".into(), "moved.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(fs.read_file(Path::new("/moved.txt")).is_ok());
        assert!(!fs.exists(Path::new("/file1.txt")));
    }

    #[test]
    fn mv_into_directory() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = MvCommand.execute(&["file1.txt".into(), "dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(fs.read_file(Path::new("/dir1/file1.txt")).is_ok());
    }

    #[test]
    fn mv_missing_operand() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = MvCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── rm tests ─────────────────────────────────────────────────────

    #[test]
    fn rm_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RmCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(!fs.exists(Path::new("/file1.txt")));
    }

    #[test]
    fn rm_force_nonexistent() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RmCommand.execute(&["-f".into(), "nope.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
    }

    #[test]
    fn rm_dir_without_r_fails() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RmCommand.execute(&["dir1".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("Is a directory"));
    }

    #[test]
    fn rm_recursive_dir() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RmCommand.execute(&["-rf".into(), "dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(!fs.exists(Path::new("/dir1")));
    }

    #[test]
    fn rm_no_args() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RmCommand.execute(&[], &c);
        assert_eq!(r.exit_code, 1);
    }

    #[test]
    fn rm_force_no_args() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = RmCommand.execute(&["-f".into()], &c);
        assert_eq!(r.exit_code, 0);
    }

    // ── tee tests ────────────────────────────────────────────────────

    #[test]
    fn tee_write_to_file_and_stdout() {
        let (fs, env, limits, np) = setup();
        let c = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "piped data",
            limits: &limits,
            network_policy: &np,
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
        let (fs, env, limits, np) = setup();
        let c = CommandContext {
            fs: &*fs,
            cwd: "/",
            env: &env,
            stdin: "more",
            limits: &limits,
            network_policy: &np,
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
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = StatCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("file1.txt"));
        assert!(r.stdout.contains("regular file"));
    }

    #[test]
    fn stat_directory() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = StatCommand.execute(&["dir1".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("directory"));
    }

    #[test]
    fn stat_nonexistent() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = StatCommand.execute(&["nope".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── chmod tests ──────────────────────────────────────────────────

    #[test]
    fn chmod_basic() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ChmodCommand.execute(&["755".into(), "file1.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        let meta = fs.stat(Path::new("/file1.txt")).unwrap();
        assert_eq!(meta.mode, 0o755);
    }

    #[test]
    fn chmod_invalid_mode() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ChmodCommand.execute(&["xyz".into(), "file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("invalid mode"));
    }

    #[test]
    fn chmod_missing_operand() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = ChmodCommand.execute(&["755".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── ln tests ─────────────────────────────────────────────────────

    #[test]
    fn ln_symbolic() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = LnCommand.execute(&["-s".into(), "file1.txt".into(), "link.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        let meta = fs.lstat(Path::new("/link.txt")).unwrap();
        assert_eq!(meta.node_type, NodeType::Symlink);
    }

    #[test]
    fn ln_hard() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = LnCommand.execute(&["file1.txt".into(), "hard.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/hard.txt")).unwrap(), b"hello\n");
    }

    #[test]
    fn ln_missing_operand() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = LnCommand.execute(&["file1.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
    }

    // ── double-dash tests ────────────────────────────────────────────

    #[test]
    fn cp_double_dash() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = CpCommand.execute(&["--".into(), "file1.txt".into(), "dd.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert_eq!(fs.read_file(Path::new("/dd.txt")).unwrap(), b"hello\n");
    }
}
