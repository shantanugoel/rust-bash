//! Compression and archiving commands: gzip, gunzip, zcat, tar

use super::{CommandMeta, FlagInfo, FlagStatus};
use crate::commands::{CommandContext, CommandResult};
use flate2::Compression;
use flate2::read::{GzDecoder, GzEncoder};
use std::io::Read;
use std::path::{Path, PathBuf};

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

/// Normalize a path by resolving `.` and `..` components without filesystem access.
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

/// Convert SystemTime to seconds since UNIX epoch for tar headers.
fn system_time_to_secs(t: std::time::SystemTime) -> u64 {
    t.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ── gzip ─────────────────────────────────────────────────────────────

pub struct GzipCommand;

static GZIP_META: CommandMeta = CommandMeta {
    name: "gzip",
    synopsis: "gzip [-dcfk] [-1...-9] [FILE...]",
    description: "Compress or decompress files using gzip format.",
    options: &[
        ("-d", "decompress (same as gunzip)"),
        ("-c", "write to stdout, keep original files"),
        ("-f", "force overwrite of output files"),
        ("-k", "keep original files"),
        ("-1...-9", "compression level (1=fast, 9=best)"),
    ],
    supports_help_flag: true,
    flags: &[
        FlagInfo {
            flag: "-d",
            description: "decompress",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-c",
            description: "write to stdout",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-f",
            description: "force overwrite",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-k",
            description: "keep original files",
            status: FlagStatus::Supported,
        },
    ],
};

impl super::VirtualCommand for GzipCommand {
    fn name(&self) -> &str {
        "gzip"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&GZIP_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        gzip_execute(args, ctx, false, false)
    }
}

/// Shared implementation for gzip/gunzip/zcat.
fn gzip_execute(
    args: &[String],
    ctx: &CommandContext,
    force_decompress: bool,
    force_stdout: bool,
) -> CommandResult {
    let mut decompress = force_decompress;
    let mut to_stdout = force_stdout;
    let mut keep = false;
    let mut force = false;
    let mut level: u32 = 6; // default compression level
    let mut files: Vec<&str> = Vec::new();
    let mut opts_done = false;

    for arg in args {
        if !opts_done && arg == "--" {
            opts_done = true;
            continue;
        }
        if !opts_done && arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
            for c in arg[1..].chars() {
                match c {
                    'd' => decompress = true,
                    'c' => to_stdout = true,
                    'f' => force = true,
                    'k' => keep = true,
                    '1'..='9' => level = c.to_digit(10).unwrap(),
                    _ => {
                        return CommandResult {
                            stderr: format!("gzip: invalid option -- '{}'\n", c),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                }
            }
        } else {
            files.push(arg);
        }
    }

    // No files: process stdin
    if files.is_empty() {
        return if decompress {
            gzip_decompress_stdin(ctx, to_stdout)
        } else {
            gzip_compress_stdin(ctx, level)
        };
    }

    // Process files
    let mut stderr = String::new();
    let mut stdout = String::new();
    let mut stdout_bytes: Option<Vec<u8>> = None;
    let mut exit_code = 0;

    for file in &files {
        let result = if decompress {
            gzip_decompress_file(file, ctx, to_stdout, keep, force)
        } else {
            gzip_compress_file(file, ctx, to_stdout, keep, force, level)
        };
        match result {
            Ok((out, bytes_out)) => {
                if to_stdout {
                    if let Some(bytes) = bytes_out {
                        // Accumulate binary output
                        stdout_bytes
                            .get_or_insert_with(Vec::new)
                            .extend_from_slice(&bytes);
                    }
                    stdout.push_str(&out);
                }
            }
            Err(msg) => {
                stderr.push_str(&msg);
                exit_code = 1;
            }
        }
    }

    CommandResult {
        stdout,
        stderr,
        exit_code,
        stdout_bytes,
    }
}

/// Compress stdin data and output as binary bytes.
fn gzip_compress_stdin(ctx: &CommandContext, level: u32) -> CommandResult {
    let input = if let Some(bytes) = ctx.stdin_bytes {
        bytes.to_vec()
    } else {
        ctx.stdin.as_bytes().to_vec()
    };

    let mut encoder = GzEncoder::new(&input[..], Compression::new(level));
    let mut compressed = Vec::new();
    if let Err(e) = encoder.read_to_end(&mut compressed) {
        return CommandResult {
            stderr: format!("gzip: {}\n", e),
            exit_code: 1,
            ..Default::default()
        };
    }

    CommandResult {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        stdout_bytes: Some(compressed),
    }
}

/// Decompress gzip stdin data and output as text.
fn gzip_decompress_stdin(ctx: &CommandContext, _to_stdout: bool) -> CommandResult {
    let input = if let Some(bytes) = ctx.stdin_bytes {
        bytes.to_vec()
    } else {
        ctx.stdin.as_bytes().to_vec()
    };

    let mut decoder = GzDecoder::new(&input[..]);
    // TODO: Add decompression size limit to guard against gzip bombs
    let mut decompressed = Vec::new();
    if let Err(e) = decoder.read_to_end(&mut decompressed) {
        return CommandResult {
            stderr: format!("gzip: {}\n", e),
            exit_code: 1,
            ..Default::default()
        };
    }

    CommandResult {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 0,
        stdout_bytes: Some(decompressed),
    }
}

/// Compress a file to .gz in VirtualFs.
fn gzip_compress_file(
    file: &str,
    ctx: &CommandContext,
    to_stdout: bool,
    keep: bool,
    force: bool,
    level: u32,
) -> Result<(String, Option<Vec<u8>>), String> {
    let path = resolve_path(file, ctx.cwd);
    let gz_path_str = format!("{}.gz", path.display());
    let gz_path = Path::new(&gz_path_str);

    // Read source file
    let data = ctx
        .fs
        .read_file(&path)
        .map_err(|e| format!("gzip: {}: {}\n", file, e))?;

    // Compress
    let mut encoder = GzEncoder::new(&data[..], Compression::new(level));
    let mut compressed = Vec::new();
    encoder
        .read_to_end(&mut compressed)
        .map_err(|e| format!("gzip: {}: {}\n", file, e))?;

    if to_stdout {
        return Ok((String::new(), Some(compressed)));
    }

    // Check if output exists
    if !force && ctx.fs.exists(gz_path) {
        return Err(format!(
            "gzip: {}: already exists; not overwriting\n",
            gz_path_str
        ));
    }

    // Write compressed file
    ctx.fs
        .write_file(gz_path, &compressed)
        .map_err(|e| format!("gzip: {}: {}\n", gz_path_str, e))?;

    // Remove original unless -k
    if !keep {
        ctx.fs
            .remove_file(&path)
            .map_err(|e| format!("gzip: {}: {}\n", file, e))?;
    }

    Ok((String::new(), None))
}

/// Decompress a .gz file in VirtualFs.
fn gzip_decompress_file(
    file: &str,
    ctx: &CommandContext,
    to_stdout: bool,
    keep: bool,
    force: bool,
) -> Result<(String, Option<Vec<u8>>), String> {
    let path = resolve_path(file, ctx.cwd);

    // Read compressed file
    let data = ctx
        .fs
        .read_file(&path)
        .map_err(|e| format!("gzip: {}: {}\n", file, e))?;

    // Decompress
    let mut decoder = GzDecoder::new(&data[..]);
    let mut decompressed = Vec::new();
    decoder
        .read_to_end(&mut decompressed)
        .map_err(|e| format!("gzip: {}: {}\n", file, e))?;

    if to_stdout {
        return Ok((String::new(), Some(decompressed)));
    }

    // Determine output path: strip .gz suffix
    let out_path_str = if let Some(stripped) = path.to_str().and_then(|s| s.strip_suffix(".gz")) {
        stripped.to_string()
    } else if let Some(stripped) = path.to_str().and_then(|s| s.strip_suffix(".tgz")) {
        format!("{}.tar", stripped)
    } else {
        return Err(format!("gzip: {}: unknown suffix -- ignored\n", file));
    };
    let out_path = PathBuf::from(&out_path_str);

    // Check if output exists
    if !force && ctx.fs.exists(&out_path) {
        return Err(format!(
            "gzip: {}: already exists; not overwriting\n",
            out_path_str
        ));
    }

    // Write decompressed file
    ctx.fs
        .write_file(&out_path, &decompressed)
        .map_err(|e| format!("gzip: {}: {}\n", out_path_str, e))?;

    // Remove original unless -k
    if !keep {
        ctx.fs
            .remove_file(&path)
            .map_err(|e| format!("gzip: {}: {}\n", file, e))?;
    }

    Ok((String::new(), None))
}

// ── gunzip ───────────────────────────────────────────────────────────

pub struct GunzipCommand;

static GUNZIP_META: CommandMeta = CommandMeta {
    name: "gunzip",
    synopsis: "gunzip [-cfk] [FILE...]",
    description: "Decompress gzip files.",
    options: &[
        ("-c", "write to stdout, keep original files"),
        ("-f", "force overwrite of output files"),
        ("-k", "keep .gz files"),
    ],
    supports_help_flag: true,
    flags: &[
        FlagInfo {
            flag: "-c",
            description: "write to stdout",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-f",
            description: "force overwrite",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-k",
            description: "keep .gz files",
            status: FlagStatus::Supported,
        },
    ],
};

impl super::VirtualCommand for GunzipCommand {
    fn name(&self) -> &str {
        "gunzip"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&GUNZIP_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        gzip_execute(args, ctx, true, false)
    }
}

// ── zcat ─────────────────────────────────────────────────────────────

pub struct ZcatCommand;

static ZCAT_META: CommandMeta = CommandMeta {
    name: "zcat",
    synopsis: "zcat [FILE...]",
    description: "Decompress and write gzip files to stdout.",
    options: &[],
    supports_help_flag: true,
    flags: &[],
};

impl super::VirtualCommand for ZcatCommand {
    fn name(&self) -> &str {
        "zcat"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&ZCAT_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        gzip_execute(args, ctx, true, true)
    }
}

// ── tar ──────────────────────────────────────────────────────────────

pub struct TarCommand;

static TAR_META: CommandMeta = CommandMeta {
    name: "tar",
    synopsis: "tar [cxtf] [-z] [-v] [-C DIR] -f ARCHIVE [FILE...]",
    description: "Create, extract, or list tar archives.",
    options: &[
        ("c", "create a new archive"),
        ("x", "extract files from archive"),
        ("t", "list contents of archive"),
        ("-f ARCHIVE", "use archive file (- for stdin/stdout)"),
        ("-z", "filter through gzip"),
        ("-v", "verbose output"),
        ("-C DIR", "change to DIR before operation"),
    ],
    supports_help_flag: true,
    flags: &[
        FlagInfo {
            flag: "-c",
            description: "create archive",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-x",
            description: "extract archive",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-t",
            description: "list contents",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-f",
            description: "archive file",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-z",
            description: "filter through gzip",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-v",
            description: "verbose output",
            status: FlagStatus::Supported,
        },
        FlagInfo {
            flag: "-C",
            description: "change directory",
            status: FlagStatus::Supported,
        },
    ],
};

#[derive(PartialEq)]
enum TarMode {
    None,
    Create,
    Extract,
    List,
}

impl super::VirtualCommand for TarCommand {
    fn name(&self) -> &str {
        "tar"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&TAR_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let mut mode = TarMode::None;
        let mut gzip = false;
        let mut verbose = false;
        let mut archive_file: Option<String> = None;
        let mut change_dir: Option<String> = None;
        let mut files: Vec<String> = Vec::new();

        // Parse args: tar supports both bundled flags (czvf) and separate flags (-c -z -v -f)
        let mut i = 0;
        let mut first_arg_parsed = false;
        while i < args.len() {
            let arg = &args[i];

            // First non-flag argument or argument starting with - is parsed as flags
            if !first_arg_parsed
                && !arg.starts_with('-')
                && arg.chars().all(|c| "cxtfzvC".contains(c))
            {
                // Bundled flags without leading dash (e.g., "czvf")
                first_arg_parsed = true;
                let mut chars = arg.chars().peekable();
                while let Some(c) = chars.next() {
                    match c {
                        'c' => mode = TarMode::Create,
                        'x' => mode = TarMode::Extract,
                        't' => mode = TarMode::List,
                        'z' => gzip = true,
                        'v' => verbose = true,
                        'f' => {
                            // If more chars follow, they're part of the filename
                            if chars.peek().is_some() {
                                let rest: String = chars.collect();
                                archive_file = Some(rest);
                                break;
                            }
                            // Otherwise next arg is the filename
                            i += 1;
                            if i < args.len() {
                                archive_file = Some(args[i].clone());
                            } else {
                                return CommandResult {
                                    stderr: "tar: option requires an argument -- 'f'\n".to_string(),
                                    exit_code: 2,
                                    ..Default::default()
                                };
                            }
                        }
                        'C' => {
                            i += 1;
                            if i < args.len() {
                                change_dir = Some(args[i].clone());
                            } else {
                                return CommandResult {
                                    stderr: "tar: option requires an argument -- 'C'\n".to_string(),
                                    exit_code: 2,
                                    ..Default::default()
                                };
                            }
                        }
                        _ => {
                            return CommandResult {
                                stderr: format!("tar: unknown option -- '{}'\n", c),
                                exit_code: 2,
                                ..Default::default()
                            };
                        }
                    }
                }
                i += 1;
                continue;
            }

            if arg.starts_with('-') && arg.len() > 1 && !arg.starts_with("--") {
                first_arg_parsed = true;
                let mut chars = arg[1..].chars().peekable();
                while let Some(c) = chars.next() {
                    match c {
                        'c' => mode = TarMode::Create,
                        'x' => mode = TarMode::Extract,
                        't' => mode = TarMode::List,
                        'z' => gzip = true,
                        'v' => verbose = true,
                        'f' => {
                            if chars.peek().is_some() {
                                let rest: String = chars.collect();
                                archive_file = Some(rest);
                                break;
                            }
                            i += 1;
                            if i < args.len() {
                                archive_file = Some(args[i].clone());
                            } else {
                                return CommandResult {
                                    stderr: "tar: option requires an argument -- 'f'\n".to_string(),
                                    exit_code: 2,
                                    ..Default::default()
                                };
                            }
                        }
                        'C' => {
                            i += 1;
                            if i < args.len() {
                                change_dir = Some(args[i].clone());
                            } else {
                                return CommandResult {
                                    stderr: "tar: option requires an argument -- 'C'\n".to_string(),
                                    exit_code: 2,
                                    ..Default::default()
                                };
                            }
                        }
                        _ => {
                            return CommandResult {
                                stderr: format!("tar: unknown option -- '{}'\n", c),
                                exit_code: 2,
                                ..Default::default()
                            };
                        }
                    }
                }
                i += 1;
                continue;
            }

            first_arg_parsed = true;
            files.push(arg.clone());
            i += 1;
        }

        if mode == TarMode::None {
            return CommandResult {
                stderr: "tar: You must specify one of the '-c', '-x', or '-t' options\n"
                    .to_string(),
                exit_code: 2,
                ..Default::default()
            };
        }

        let effective_cwd = if let Some(ref dir) = change_dir {
            let p = resolve_path(dir, ctx.cwd);
            p.to_string_lossy().to_string()
        } else {
            ctx.cwd.to_string()
        };

        match mode {
            TarMode::Create => tar_create(
                ctx,
                &effective_cwd,
                archive_file.as_deref(),
                &files,
                gzip,
                verbose,
            ),
            TarMode::Extract => {
                tar_extract(ctx, &effective_cwd, archive_file.as_deref(), gzip, verbose)
            }
            TarMode::List => tar_list(ctx, &effective_cwd, archive_file.as_deref(), gzip, verbose),
            TarMode::None => unreachable!(),
        }
    }
}

/// Recursively collect all files under a directory in VirtualFs.
fn collect_files_recursive(
    fs: &dyn crate::vfs::VirtualFs,
    base: &Path,
    prefix: &Path,
) -> Result<Vec<(PathBuf, Vec<u8>)>, String> {
    let mut result = Vec::new();
    let entries = fs
        .readdir(base)
        .map_err(|e| format!("tar: {}: {}\n", base.display(), e))?;

    for entry in entries {
        let full_path = base.join(&entry.name);
        let archive_path = prefix.join(&entry.name);

        match entry.node_type {
            crate::vfs::NodeType::File => {
                let data = fs
                    .read_file(&full_path)
                    .map_err(|e| format!("tar: {}: {}\n", full_path.display(), e))?;
                result.push((archive_path, data));
            }
            crate::vfs::NodeType::Directory => {
                // Emit directory entry (empty sentinel with trailing /)
                let mut dir_path = archive_path.clone();
                let dir_name = format!("{}/", dir_path.display());
                dir_path = PathBuf::from(dir_name);
                result.push((dir_path, Vec::new()));
                let sub = collect_files_recursive(fs, &full_path, &archive_path)?;
                result.extend(sub);
            }
            crate::vfs::NodeType::Symlink => {
                // TODO: Preserve symlink nature in tar (currently stored as regular file)
                if let Ok(data) = fs.read_file(&full_path) {
                    result.push((archive_path, data));
                }
            }
        }
    }
    Ok(result)
}

fn tar_create(
    ctx: &CommandContext,
    effective_cwd: &str,
    archive_file: Option<&str>,
    files: &[String],
    gzip: bool,
    verbose: bool,
) -> CommandResult {
    if files.is_empty() {
        return CommandResult {
            stderr: "tar: Cowardly refusing to create an empty archive\n".to_string(),
            exit_code: 2,
            ..Default::default()
        };
    }

    // Build tar archive in memory
    let mut tar_builder = tar::Builder::new(Vec::new());
    let mut verbose_output = String::new();
    let mut stderr = String::new();

    for file_arg in files {
        let path = resolve_path(file_arg, effective_cwd);

        if !ctx.fs.exists(&path) {
            stderr.push_str(&format!("tar: {}: No such file or directory\n", file_arg));
            continue;
        }

        let stat = match ctx.fs.stat(&path) {
            Ok(s) => s,
            Err(e) => {
                stderr.push_str(&format!("tar: {}: {}\n", file_arg, e));
                continue;
            }
        };

        if stat.node_type == crate::vfs::NodeType::Directory {
            // Recursively add directory contents
            let entries = match collect_files_recursive(ctx.fs, &path, Path::new(file_arg)) {
                Ok(e) => e,
                Err(msg) => {
                    stderr.push_str(&msg);
                    continue;
                }
            };

            // Add directory entry itself
            let mut dir_header = tar::Header::new_gnu();
            dir_header.set_entry_type(tar::EntryType::Directory);
            dir_header.set_size(0);
            dir_header.set_mode(0o755);
            dir_header.set_mtime(system_time_to_secs(stat.mtime));
            let dir_name = format!("{}/", file_arg);
            dir_header.set_cksum();
            if tar_builder
                .append_data(&mut dir_header, &dir_name, &[][..])
                .is_err()
            {
                stderr.push_str(&format!("tar: error writing {}\n", dir_name));
                continue;
            }
            if verbose {
                verbose_output.push_str(&format!("{}\n", dir_name));
            }

            for (archive_path, data) in entries {
                let archive_name = archive_path.to_string_lossy().to_string();

                // Directory sentinel: path ends with / and data is empty
                if archive_name.ends_with('/') && data.is_empty() {
                    let mut header = tar::Header::new_gnu();
                    header.set_entry_type(tar::EntryType::Directory);
                    header.set_size(0);
                    header.set_mode(0o755);
                    header.set_mtime(0);
                    header.set_cksum();
                    if tar_builder
                        .append_data(&mut header, &archive_name, &[][..])
                        .is_err()
                    {
                        stderr.push_str(&format!("tar: error writing {}\n", archive_name));
                    }
                    if verbose {
                        verbose_output.push_str(&format!("{}\n", archive_name));
                    }
                    continue;
                }

                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(0o644);
                header.set_mtime(0);
                header.set_cksum();

                let archive_name = archive_path.to_string_lossy().to_string();
                if tar_builder
                    .append_data(&mut header, &archive_name, &data[..])
                    .is_err()
                {
                    stderr.push_str(&format!("tar: error writing {}\n", archive_name));
                    continue;
                }
                if verbose {
                    verbose_output.push_str(&format!("{}\n", archive_name));
                }
            }
        } else {
            // Single file
            let data = match ctx.fs.read_file(&path) {
                Ok(d) => d,
                Err(e) => {
                    stderr.push_str(&format!("tar: {}: {}\n", file_arg, e));
                    continue;
                }
            };

            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_mtime(system_time_to_secs(stat.mtime));
            header.set_cksum();

            if tar_builder
                .append_data(&mut header, file_arg, &data[..])
                .is_err()
            {
                stderr.push_str(&format!("tar: error writing {}\n", file_arg));
                continue;
            }
            if verbose {
                verbose_output.push_str(&format!("{}\n", file_arg));
            }
        }
    }

    // Finalize
    let tar_data = match tar_builder.into_inner() {
        Ok(d) => d,
        Err(e) => {
            return CommandResult {
                stderr: format!("tar: {}\n", e),
                exit_code: 1,
                ..Default::default()
            };
        }
    };

    // Optionally compress with gzip
    let final_data = if gzip {
        let mut encoder = GzEncoder::new(&tar_data[..], Compression::default());
        let mut compressed = Vec::new();
        if let Err(e) = encoder.read_to_end(&mut compressed) {
            return CommandResult {
                stderr: format!("tar: gzip compression failed: {}\n", e),
                exit_code: 1,
                ..Default::default()
            };
        }
        compressed
    } else {
        tar_data
    };

    // Write to file or stdout
    let has_errors = !stderr.is_empty();
    match archive_file {
        Some("-") | None => {
            // Output to stdout as binary
            CommandResult {
                stdout: verbose_output,
                stderr,
                exit_code: i32::from(has_errors),
                stdout_bytes: Some(final_data),
            }
        }
        Some(name) => {
            let path = resolve_path(name, ctx.cwd);
            if let Err(e) = ctx.fs.write_file(&path, &final_data) {
                return CommandResult {
                    stderr: format!("tar: {}: {}\n", name, e),
                    exit_code: 1,
                    ..Default::default()
                };
            }
            CommandResult {
                stdout: verbose_output,
                stderr,
                exit_code: i32::from(has_errors),
                stdout_bytes: None,
            }
        }
    }
}

fn tar_extract(
    ctx: &CommandContext,
    effective_cwd: &str,
    archive_file: Option<&str>,
    gzip: bool,
    verbose: bool,
) -> CommandResult {
    // Read archive data
    let archive_data = match archive_file {
        Some("-") | None => {
            if let Some(bytes) = ctx.stdin_bytes {
                bytes.to_vec()
            } else {
                ctx.stdin.as_bytes().to_vec()
            }
        }
        Some(name) => {
            let path = resolve_path(name, ctx.cwd);
            match ctx.fs.read_file(&path) {
                Ok(d) => d,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("tar: {}: {}\n", name, e),
                        exit_code: 1,
                        ..Default::default()
                    };
                }
            }
        }
    };

    // Optionally decompress gzip
    let tar_data = if gzip {
        let mut decoder = GzDecoder::new(&archive_data[..]);
        let mut decompressed = Vec::new();
        if let Err(e) = decoder.read_to_end(&mut decompressed) {
            return CommandResult {
                stderr: format!("tar: gzip decompression failed: {}\n", e),
                exit_code: 1,
                ..Default::default()
            };
        }
        decompressed
    } else {
        archive_data
    };

    // Extract entries
    let mut archive = tar::Archive::new(&tar_data[..]);
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => {
            return CommandResult {
                stderr: format!("tar: {}\n", e),
                exit_code: 1,
                ..Default::default()
            };
        }
    };

    let mut verbose_output = String::new();
    let mut stderr = String::new();

    for entry_result in entries {
        let mut entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                stderr.push_str(&format!("tar: {}\n", e));
                continue;
            }
        };

        let entry_path = match entry.path() {
            Ok(p) => p.to_path_buf(),
            Err(e) => {
                stderr.push_str(&format!("tar: {}\n", e));
                continue;
            }
        };

        let full_path = resolve_path(&entry_path.to_string_lossy(), effective_cwd);

        // Guard against path-traversal attacks (e.g., entries like "../../bin/ls")
        let normalized = normalize_path(&full_path);
        let normalized_str = normalized.to_string_lossy();
        let norm_cwd = if effective_cwd.ends_with('/') {
            effective_cwd.to_string()
        } else {
            format!("{}/", effective_cwd)
        };
        if !normalized_str.starts_with(&norm_cwd) && *normalized_str != *effective_cwd {
            stderr.push_str(&format!(
                "tar: {}: path escapes extraction directory, skipping\n",
                entry_path.display()
            ));
            continue;
        }

        if verbose {
            verbose_output.push_str(&format!("{}\n", entry_path.display()));
        }

        match entry.header().entry_type() {
            tar::EntryType::Directory => {
                if let Err(e) = ctx.fs.mkdir_p(&full_path) {
                    stderr.push_str(&format!("tar: {}: {}\n", entry_path.display(), e));
                }
            }
            tar::EntryType::Regular | tar::EntryType::GNUSparse => {
                // Ensure parent directory exists
                if let Some(parent) = full_path.parent()
                    && !ctx.fs.exists(parent)
                {
                    let _ = ctx.fs.mkdir_p(parent);
                }

                let mut data = Vec::new();
                if let Err(e) = entry.read_to_end(&mut data) {
                    stderr.push_str(&format!("tar: {}: {}\n", entry_path.display(), e));
                    continue;
                }

                if let Err(e) = ctx.fs.write_file(&full_path, &data) {
                    stderr.push_str(&format!("tar: {}: {}\n", entry_path.display(), e));
                }
            }
            _ => {
                // Skip other entry types (symlinks, etc.) - read data to advance cursor
                let mut data = Vec::new();
                let _ = entry.read_to_end(&mut data);
                // Try to write as a regular file
                if let Some(parent) = full_path.parent()
                    && !ctx.fs.exists(parent)
                {
                    let _ = ctx.fs.mkdir_p(parent);
                }
                let _ = ctx.fs.write_file(&full_path, &data);
            }
        }
    }

    let has_errors = !stderr.is_empty();
    CommandResult {
        stdout: verbose_output,
        stderr,
        exit_code: i32::from(has_errors),
        stdout_bytes: None,
    }
}

fn tar_list(
    ctx: &CommandContext,
    _effective_cwd: &str,
    archive_file: Option<&str>,
    gzip: bool,
    verbose: bool,
) -> CommandResult {
    // Read archive data
    let archive_data = match archive_file {
        Some("-") | None => {
            if let Some(bytes) = ctx.stdin_bytes {
                bytes.to_vec()
            } else {
                ctx.stdin.as_bytes().to_vec()
            }
        }
        Some(name) => {
            let path = resolve_path(name, ctx.cwd);
            match ctx.fs.read_file(&path) {
                Ok(d) => d,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("tar: {}: {}\n", name, e),
                        exit_code: 1,
                        ..Default::default()
                    };
                }
            }
        }
    };

    // Optionally decompress gzip
    let tar_data = if gzip {
        let mut decoder = GzDecoder::new(&archive_data[..]);
        let mut decompressed = Vec::new();
        if let Err(e) = decoder.read_to_end(&mut decompressed) {
            return CommandResult {
                stderr: format!("tar: gzip decompression failed: {}\n", e),
                exit_code: 1,
                ..Default::default()
            };
        }
        decompressed
    } else {
        archive_data
    };

    let mut archive = tar::Archive::new(&tar_data[..]);
    let entries = match archive.entries() {
        Ok(e) => e,
        Err(e) => {
            return CommandResult {
                stderr: format!("tar: {}\n", e),
                exit_code: 1,
                ..Default::default()
            };
        }
    };

    let mut output = String::new();

    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                return CommandResult {
                    stderr: format!("tar: {}\n", e),
                    exit_code: 1,
                    ..Default::default()
                };
            }
        };

        let path = match entry.path() {
            Ok(p) => p.to_path_buf(),
            Err(e) => {
                return CommandResult {
                    stderr: format!("tar: {}\n", e),
                    exit_code: 1,
                    ..Default::default()
                };
            }
        };

        if verbose {
            let header = entry.header();
            let mode = header.mode().unwrap_or(0);
            let size = header.size().unwrap_or(0);
            output.push_str(&format!("{:o} {:>8} {}\n", mode, size, path.display()));
        } else {
            output.push_str(&format!("{}\n", path.display()));
        }
    }

    CommandResult {
        stdout: output,
        stderr: String::new(),
        exit_code: 0,
        stdout_bytes: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::VirtualCommand;
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
        (
            Arc::new(InMemoryFs::new()),
            HashMap::new(),
            ExecutionLimits::default(),
            NetworkPolicy::default(),
        )
    }

    fn ctx<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        np: &'a NetworkPolicy,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin: "",
            stdin_bytes: None,
            limits,
            network_policy: np,
            exec: None,
        }
    }

    fn ctx_with_stdin_bytes<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        np: &'a NetworkPolicy,
        stdin: &'a str,
        stdin_bytes: Option<&'a [u8]>,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin,
            stdin_bytes,
            limits,
            network_policy: np,
            exec: None,
        }
    }

    // ── gzip tests ──────────────────────────────────────────────────

    #[test]
    fn gzip_compress_file_creates_gz() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.txt"), b"hello world\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GzipCommand.execute(&["test.txt".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        assert!(fs.exists(Path::new("/test.txt.gz")));
        assert!(!fs.exists(Path::new("/test.txt"))); // original removed
    }

    #[test]
    fn gzip_keep_original() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.txt"), b"hello world\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GzipCommand.execute(&["-k".into(), "test.txt".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        assert!(fs.exists(Path::new("/test.txt.gz")));
        assert!(fs.exists(Path::new("/test.txt"))); // original kept
    }

    #[test]
    fn gzip_decompress_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.txt"), b"hello world\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        // Compress first
        GzipCommand.execute(&["test.txt".into()], &c);
        assert!(fs.exists(Path::new("/test.txt.gz")));

        // Decompress
        let r = GzipCommand.execute(&["-d".into(), "test.txt.gz".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        assert!(fs.exists(Path::new("/test.txt")));
        assert!(!fs.exists(Path::new("/test.txt.gz")));

        let content = fs.read_file(Path::new("/test.txt")).unwrap();
        assert_eq!(content, b"hello world\n");
    }

    #[test]
    fn gzip_to_stdout() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.txt"), b"hello world\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GzipCommand.execute(&["-c".into(), "test.txt".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        assert!(r.stdout_bytes.is_some());
        assert!(fs.exists(Path::new("/test.txt"))); // original kept with -c
    }

    #[test]
    fn gzip_stdin_compress_decompress_roundtrip() {
        let (fs, env, limits, np) = setup();
        let input = "hello binary world\n";

        // Compress from stdin
        let c = ctx_with_stdin_bytes(&*fs, &env, &limits, &np, input, None);
        let compressed = GzipCommand.execute(&[], &c);
        assert_eq!(compressed.exit_code, 0, "stderr: {}", compressed.stderr);
        assert!(compressed.stdout_bytes.is_some());

        // Decompress from stdin_bytes (simulating pipeline)
        let bytes = compressed.stdout_bytes.unwrap();
        let c2 = ctx_with_stdin_bytes(&*fs, &env, &limits, &np, "", Some(&bytes));
        let decompressed = GunzipCommand.execute(&[], &c2);
        assert_eq!(decompressed.exit_code, 0, "stderr: {}", decompressed.stderr);
        let output = decompressed
            .stdout_bytes
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or(decompressed.stdout);
        assert_eq!(output, input);
    }

    #[test]
    fn gunzip_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.txt"), b"hello\n").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        GzipCommand.execute(&["test.txt".into()], &c);
        let r = GunzipCommand.execute(&["test.txt.gz".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        assert!(fs.exists(Path::new("/test.txt")));
        let content = fs.read_file(Path::new("/test.txt")).unwrap();
        assert_eq!(content, b"hello\n");
    }

    #[test]
    fn zcat_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.txt"), b"zcat test\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        GzipCommand.execute(&["-k".into(), "test.txt".into()], &c);
        let r = ZcatCommand.execute(&["test.txt.gz".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        let output = r
            .stdout_bytes
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or(r.stdout);
        assert_eq!(output, "zcat test\n");
        assert!(fs.exists(Path::new("/test.txt.gz"))); // not removed
    }

    #[test]
    fn gzip_nonexistent_file() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = GzipCommand.execute(&["nonexistent.txt".into()], &c);
        assert_eq!(r.exit_code, 1);
        assert!(r.stderr.contains("nonexistent.txt"));
    }

    #[test]
    fn gzip_compression_levels() {
        let (fs, env, limits, np) = setup();
        let data = "a".repeat(1000);
        fs.write_file(Path::new("/test.txt"), data.as_bytes())
            .unwrap();

        // Fast compression
        let c = ctx(&*fs, &env, &limits, &np);
        let r1 = GzipCommand.execute(&["-c".into(), "-1".into(), "test.txt".into()], &c);
        assert_eq!(r1.exit_code, 0);
        let fast_size = r1.stdout_bytes.as_ref().unwrap().len();

        // Best compression
        let r9 = GzipCommand.execute(&["-c".into(), "-9".into(), "test.txt".into()], &c);
        assert_eq!(r9.exit_code, 0);
        let best_size = r9.stdout_bytes.as_ref().unwrap().len();

        // Best should be <= fast
        assert!(best_size <= fast_size);
    }

    // ── tar tests ───────────────────────────────────────────────────

    #[test]
    fn tar_create_and_extract_file() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/hello.txt"), b"hello world\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        // Create archive
        let r = TarCommand.execute(&["cf".into(), "archive.tar".into(), "hello.txt".into()], &c);
        assert_eq!(r.exit_code, 0, "create stderr: {}", r.stderr);
        assert!(fs.exists(Path::new("/archive.tar")));

        // Remove original
        fs.remove_file(Path::new("/hello.txt")).unwrap();

        // Extract
        let r = TarCommand.execute(&["xf".into(), "archive.tar".into()], &c);
        assert_eq!(r.exit_code, 0, "extract stderr: {}", r.stderr);
        assert!(fs.exists(Path::new("/hello.txt")));

        let content = fs.read_file(Path::new("/hello.txt")).unwrap();
        assert_eq!(content, b"hello world\n");
    }

    #[test]
    fn tar_create_and_list() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/a.txt"), b"aaa").unwrap();
        fs.write_file(Path::new("/b.txt"), b"bbb").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        TarCommand.execute(
            &[
                "cf".into(),
                "archive.tar".into(),
                "a.txt".into(),
                "b.txt".into(),
            ],
            &c,
        );

        let r = TarCommand.execute(&["tf".into(), "archive.tar".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        assert!(r.stdout.contains("a.txt"));
        assert!(r.stdout.contains("b.txt"));
    }

    #[test]
    fn tar_create_directory() {
        let (fs, env, limits, np) = setup();
        fs.mkdir_p(Path::new("/mydir")).unwrap();
        fs.write_file(Path::new("/mydir/file1.txt"), b"content1")
            .unwrap();
        fs.write_file(Path::new("/mydir/file2.txt"), b"content2")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        TarCommand.execute(&["cf".into(), "archive.tar".into(), "mydir".into()], &c);

        // List
        let r = TarCommand.execute(&["tf".into(), "archive.tar".into()], &c);
        assert_eq!(r.exit_code, 0, "stderr: {}", r.stderr);
        assert!(r.stdout.contains("mydir/"));
        assert!(r.stdout.contains("mydir/file1.txt"));
        assert!(r.stdout.contains("mydir/file2.txt"));
    }

    #[test]
    fn tar_with_gzip() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/test.txt"), b"gzipped tar content\n")
            .unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        // Create gzipped tar
        let r = TarCommand.execute(
            &["czf".into(), "archive.tar.gz".into(), "test.txt".into()],
            &c,
        );
        assert_eq!(r.exit_code, 0, "create stderr: {}", r.stderr);

        // Remove original
        fs.remove_file(Path::new("/test.txt")).unwrap();

        // Extract
        let r = TarCommand.execute(&["xzf".into(), "archive.tar.gz".into()], &c);
        assert_eq!(r.exit_code, 0, "extract stderr: {}", r.stderr);

        let content = fs.read_file(Path::new("/test.txt")).unwrap();
        assert_eq!(content, b"gzipped tar content\n");
    }

    #[test]
    fn tar_verbose() {
        let (fs, env, limits, np) = setup();
        fs.write_file(Path::new("/file.txt"), b"data").unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        let r = TarCommand.execute(&["cvf".into(), "archive.tar".into(), "file.txt".into()], &c);
        assert_eq!(r.exit_code, 0);
        assert!(r.stdout.contains("file.txt"));
    }

    #[test]
    fn tar_change_dir() {
        let (fs, env, limits, np) = setup();
        fs.mkdir_p(Path::new("/src")).unwrap();
        fs.write_file(Path::new("/src/code.rs"), b"fn main() {}")
            .unwrap();
        fs.mkdir_p(Path::new("/dest")).unwrap();
        let c = ctx(&*fs, &env, &limits, &np);

        // Create archive from /src
        let r = TarCommand.execute(
            &[
                "-C".into(),
                "/src".into(),
                "-cf".into(),
                "/archive.tar".into(),
                "code.rs".into(),
            ],
            &c,
        );
        assert_eq!(r.exit_code, 0, "create stderr: {}", r.stderr);

        // Extract to /dest
        let r = TarCommand.execute(
            &[
                "-C".into(),
                "/dest".into(),
                "-xf".into(),
                "/archive.tar".into(),
            ],
            &c,
        );
        assert_eq!(r.exit_code, 0, "extract stderr: {}", r.stderr);
        assert!(fs.exists(Path::new("/dest/code.rs")));
    }

    #[test]
    fn tar_no_mode_specified() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TarCommand.execute(&["-f".into(), "test.tar".into()], &c);
        assert_eq!(r.exit_code, 2);
        assert!(r.stderr.contains("must specify"));
    }

    #[test]
    fn tar_empty_archive_refused() {
        let (fs, env, limits, np) = setup();
        let c = ctx(&*fs, &env, &limits, &np);
        let r = TarCommand.execute(&["cf".into(), "empty.tar".into()], &c);
        assert_eq!(r.exit_code, 2);
        assert!(r.stderr.contains("empty archive"));
    }
}
