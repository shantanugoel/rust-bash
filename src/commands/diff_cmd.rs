//! diff command: compare files line by line

use crate::commands::{CommandContext, CommandMeta, CommandResult};
use crate::vfs::NodeType;
use similar::TextDiff;
use std::path::PathBuf;

pub struct DiffCommand;

#[derive(Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Normal,
    Unified(usize),
    Context(usize),
}

struct DiffOpts<'a> {
    format: OutputFormat,
    recursive: bool,
    brief: bool,
    report_identical: bool,
    new_file: bool,
    ignore_case: bool,
    ignore_all_space: bool,
    ignore_space_change: bool,
    ignore_blank_lines: bool,
    labels: Vec<&'a str>,
}

impl<'a> Default for DiffOpts<'a> {
    fn default() -> Self {
        Self {
            format: OutputFormat::Normal,
            recursive: false,
            brief: false,
            report_identical: false,
            new_file: false,
            ignore_case: false,
            ignore_all_space: false,
            ignore_space_change: false,
            ignore_blank_lines: false,
            labels: Vec::new(),
        }
    }
}

/// Holds preprocessed and original line data for a single file side.
struct DiffInput<'a> {
    orig: Vec<&'a str>,
    proc: Vec<String>,
    map: Vec<usize>,
}

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

fn preprocess_line(line: &str, opts: &DiffOpts) -> String {
    let mut s = line.to_string();
    if opts.ignore_case {
        s = s.to_lowercase();
    }
    if opts.ignore_all_space {
        s.retain(|c| !c.is_whitespace());
    } else if opts.ignore_space_change {
        let mut result = String::with_capacity(s.len());
        let mut in_space = false;
        for c in s.chars() {
            if c.is_whitespace() {
                if !in_space {
                    result.push(' ');
                    in_space = true;
                }
            } else {
                result.push(c);
                in_space = false;
            }
        }
        s = result.trim_end().to_string();
    }
    s
}

fn needs_preprocessing(opts: &DiffOpts) -> bool {
    opts.ignore_blank_lines || opts.ignore_case || opts.ignore_all_space || opts.ignore_space_change
}

/// Split content into lines (preserving line endings via split_inclusive),
/// preprocess each line for comparison, and return a `DiffInput`.
fn preprocess_lines<'a>(content: &'a str, opts: &DiffOpts) -> DiffInput<'a> {
    let orig_lines: Vec<&str> = if content.is_empty() {
        Vec::new()
    } else {
        content.split_inclusive('\n').collect()
    };

    if !needs_preprocessing(opts) {
        let proc: Vec<String> = orig_lines.iter().map(|l| l.to_string()).collect();
        let idx_map: Vec<usize> = (0..orig_lines.len()).collect();
        return DiffInput {
            orig: orig_lines,
            proc,
            map: idx_map,
        };
    }

    let mut proc_lines = Vec::new();
    let mut idx_map = Vec::new();

    for (i, line) in orig_lines.iter().enumerate() {
        if opts.ignore_blank_lines && line.trim().is_empty() {
            continue;
        }
        proc_lines.push(preprocess_line(line, opts));
        idx_map.push(i);
    }

    DiffInput {
        orig: orig_lines,
        proc: proc_lines,
        map: idx_map,
    }
}

fn read_file_content(path: &str, ctx: &CommandContext) -> Result<String, String> {
    if path == "-" {
        return Ok(ctx.stdin.to_string());
    }
    let resolved = resolve_path(path, ctx.cwd);
    match ctx.fs.read_file(&resolved) {
        Ok(bytes) => Ok(String::from_utf8_lossy(&bytes).to_string()),
        Err(e) => Err(format!("diff: {}: {}", path, e)),
    }
}

fn is_directory(path: &str, ctx: &CommandContext) -> bool {
    if path == "-" {
        return false;
    }
    let resolved = resolve_path(path, ctx.cwd);
    match ctx.fs.stat(&resolved) {
        Ok(meta) => meta.node_type == NodeType::Directory,
        Err(_) => false,
    }
}

fn file_exists(path: &str, ctx: &CommandContext) -> bool {
    if path == "-" {
        return true;
    }
    let resolved = resolve_path(path, ctx.cwd);
    ctx.fs.exists(&resolved)
}

/// Formats diff output in normal (ed-style) format.
fn format_normal_diff(a: &DiffInput, b: &DiffInput) -> String {
    let p1_refs: Vec<&str> = a.proc.iter().map(|s| s.as_str()).collect();
    let p2_refs: Vec<&str> = b.proc.iter().map(|s| s.as_str()).collect();
    let diff = TextDiff::from_slices(&p1_refs, &p2_refs);
    let mut output = String::new();

    for group in diff.grouped_ops(0) {
        let first_op = &group[0];
        let last_op = &group[group.len() - 1];

        let old_range = first_op.old_range().start..last_op.old_range().end;
        let new_range = first_op.new_range().start..last_op.new_range().end;

        // Map preprocessed indices to 1-based original line numbers
        let mapped_old_start = map_to_orig_1based(&a.map, old_range.start);
        let mapped_old_end = map_to_orig_1based(&a.map, old_range.end.saturating_sub(1));
        let mapped_new_start = map_to_orig_1based(&b.map, new_range.start);
        let mapped_new_end = map_to_orig_1based(&b.map, new_range.end.saturating_sub(1));

        // Position *before* an insert/delete point (the line before)
        let old_before = if old_range.start > 0 {
            map_to_orig_1based(&a.map, old_range.start - 1)
        } else {
            0
        };
        let new_before = if new_range.start > 0 {
            map_to_orig_1based(&b.map, new_range.start - 1)
        } else {
            0
        };

        let has_delete = group
            .iter()
            .any(|op| matches!(op, similar::DiffOp::Delete { .. }));
        let has_insert = group
            .iter()
            .any(|op| matches!(op, similar::DiffOp::Insert { .. }));
        let has_replace = group
            .iter()
            .any(|op| matches!(op, similar::DiffOp::Replace { .. }));

        if has_replace || (has_delete && has_insert) {
            let old_range_str = format_normal_range(mapped_old_start, mapped_old_end);
            let new_range_str = format_normal_range(mapped_new_start, mapped_new_end);
            output.push_str(&format!("{}c{}\n", old_range_str, new_range_str));
        } else if has_delete {
            let old_range_str = format_normal_range(mapped_old_start, mapped_old_end);
            output.push_str(&format!("{}d{}\n", old_range_str, new_before));
        } else {
            let new_range_str = format_normal_range(mapped_new_start, mapped_new_end);
            output.push_str(&format!("{}a{}\n", old_before, new_range_str));
        }

        for op in &group {
            let op_old = op.old_range();
            for idx in op_old.clone() {
                if matches!(
                    op,
                    similar::DiffOp::Delete { .. } | similar::DiffOp::Replace { .. }
                ) {
                    let orig_idx = a.map[idx];
                    output.push_str(&format!("< {}", a.orig[orig_idx]));
                    if !a.orig[orig_idx].ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
        }

        if has_replace || (has_delete && has_insert) {
            output.push_str("---\n");
        }

        for op in &group {
            let op_new = op.new_range();
            for idx in op_new.clone() {
                if matches!(
                    op,
                    similar::DiffOp::Insert { .. } | similar::DiffOp::Replace { .. }
                ) {
                    let orig_idx = b.map[idx];
                    output.push_str(&format!("> {}", b.orig[orig_idx]));
                    if !b.orig[orig_idx].ends_with('\n') {
                        output.push('\n');
                    }
                }
            }
        }
    }

    output
}

/// Map a preprocessed line index to a 1-based original line number.
fn map_to_orig_1based(map: &[usize], idx: usize) -> usize {
    if idx < map.len() {
        map[idx] + 1
    } else if !map.is_empty() {
        map[map.len() - 1] + 1
    } else {
        0
    }
}

fn format_normal_range(start: usize, end: usize) -> String {
    if start == end {
        format!("{}", start)
    } else {
        format!("{},{}", start, end)
    }
}

fn format_unified_diff(
    a: &DiffInput,
    b: &DiffInput,
    old_label: &str,
    new_label: &str,
    context: usize,
) -> String {
    let p1_refs: Vec<&str> = a.proc.iter().map(|s| s.as_str()).collect();
    let p2_refs: Vec<&str> = b.proc.iter().map(|s| s.as_str()).collect();
    let diff = TextDiff::from_slices(&p1_refs, &p2_refs);
    let mut output = String::new();

    output.push_str(&format!("--- {}\n", old_label));
    output.push_str(&format!("+++ {}\n", new_label));

    for group in diff.grouped_ops(context) {
        let first_op = &group[0];
        let last_op = &group[group.len() - 1];

        let old_range = first_op.old_range().start..last_op.old_range().end;
        let new_range = first_op.new_range().start..last_op.new_range().end;

        let old_start = if old_range.start < a.map.len() {
            a.map[old_range.start] + 1
        } else {
            1
        };
        let old_count = if old_range.end > old_range.start {
            a.map[old_range.end - 1] - a.map[old_range.start] + 1
        } else {
            0
        };
        let new_start = if new_range.start < b.map.len() {
            b.map[new_range.start] + 1
        } else {
            1
        };
        let new_count = if new_range.end > new_range.start {
            b.map[new_range.end - 1] - b.map[new_range.start] + 1
        } else {
            0
        };

        output.push_str(&format!(
            "@@ -{},{} +{},{} @@\n",
            old_start, old_count, new_start, new_count
        ));

        for op in &group {
            match op {
                similar::DiffOp::Equal { old_index, len, .. } => {
                    for i in 0..*len {
                        let orig_idx = a.map[old_index + i];
                        output.push(' ');
                        output.push_str(a.orig[orig_idx]);
                        if !a.orig[orig_idx].ends_with('\n') {
                            output.push('\n');
                        }
                    }
                }
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                } => {
                    for i in 0..*old_len {
                        let orig_idx = a.map[old_index + i];
                        output.push('-');
                        output.push_str(a.orig[orig_idx]);
                        if !a.orig[orig_idx].ends_with('\n') {
                            output.push('\n');
                        }
                    }
                }
                similar::DiffOp::Insert {
                    new_index, new_len, ..
                } => {
                    for i in 0..*new_len {
                        let orig_idx = b.map[new_index + i];
                        output.push('+');
                        output.push_str(b.orig[orig_idx]);
                        if !b.orig[orig_idx].ends_with('\n') {
                            output.push('\n');
                        }
                    }
                }
                similar::DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    for i in 0..*old_len {
                        let orig_idx = a.map[old_index + i];
                        output.push('-');
                        output.push_str(a.orig[orig_idx]);
                        if !a.orig[orig_idx].ends_with('\n') {
                            output.push('\n');
                        }
                    }
                    for i in 0..*new_len {
                        let orig_idx = b.map[new_index + i];
                        output.push('+');
                        output.push_str(b.orig[orig_idx]);
                        if !b.orig[orig_idx].ends_with('\n') {
                            output.push('\n');
                        }
                    }
                }
            }
        }
    }

    output
}

fn format_context_diff(
    a: &DiffInput,
    b: &DiffInput,
    old_label: &str,
    new_label: &str,
    context: usize,
) -> String {
    let p1_refs: Vec<&str> = a.proc.iter().map(|s| s.as_str()).collect();
    let p2_refs: Vec<&str> = b.proc.iter().map(|s| s.as_str()).collect();
    let diff = TextDiff::from_slices(&p1_refs, &p2_refs);
    let mut output = String::new();

    output.push_str(&format!("*** {}\n", old_label));
    output.push_str(&format!("--- {}\n", new_label));

    for group in diff.grouped_ops(context) {
        let old_range = group[0].old_range().start..group[group.len() - 1].old_range().end;
        let new_range = group[0].new_range().start..group[group.len() - 1].new_range().end;

        let old_start = if old_range.start < a.map.len() {
            a.map[old_range.start] + 1
        } else {
            1
        };
        let old_end = if old_range.end > 0 && old_range.end - 1 < a.map.len() {
            a.map[old_range.end - 1] + 1
        } else {
            old_start
        };
        let new_start = if new_range.start < b.map.len() {
            b.map[new_range.start] + 1
        } else {
            1
        };
        let new_end = if new_range.end > 0 && new_range.end - 1 < b.map.len() {
            b.map[new_range.end - 1] + 1
        } else {
            new_start
        };

        output.push_str("***************\n");

        output.push_str(&format!(
            "*** {},{} ****\n",
            old_start,
            old_end.max(old_start)
        ));
        let has_old_changes = group.iter().any(|op| {
            matches!(
                op,
                similar::DiffOp::Delete { .. } | similar::DiffOp::Replace { .. }
            )
        });
        if has_old_changes {
            for op in &group {
                match op {
                    similar::DiffOp::Equal { old_index, len, .. } => {
                        for i in 0..*len {
                            let orig_idx = a.map[old_index + i];
                            output.push_str(&format!("  {}", a.orig[orig_idx]));
                            if !a.orig[orig_idx].ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                    similar::DiffOp::Delete {
                        old_index, old_len, ..
                    } => {
                        for i in 0..*old_len {
                            let orig_idx = a.map[old_index + i];
                            output.push_str(&format!("- {}", a.orig[orig_idx]));
                            if !a.orig[orig_idx].ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                    similar::DiffOp::Replace {
                        old_index, old_len, ..
                    } => {
                        for i in 0..*old_len {
                            let orig_idx = a.map[old_index + i];
                            output.push_str(&format!("! {}", a.orig[orig_idx]));
                            if !a.orig[orig_idx].ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        output.push_str(&format!(
            "--- {},{} ----\n",
            new_start,
            new_end.max(new_start)
        ));
        let has_new_changes = group.iter().any(|op| {
            matches!(
                op,
                similar::DiffOp::Insert { .. } | similar::DiffOp::Replace { .. }
            )
        });
        if has_new_changes {
            for op in &group {
                match op {
                    similar::DiffOp::Equal { old_index, len, .. } => {
                        for i in 0..*len {
                            let orig_idx = a.map[old_index + i];
                            output.push_str(&format!("  {}", a.orig[orig_idx]));
                            if !a.orig[orig_idx].ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                    similar::DiffOp::Insert {
                        new_index, new_len, ..
                    } => {
                        for i in 0..*new_len {
                            let orig_idx = b.map[new_index + i];
                            output.push_str(&format!("+ {}", b.orig[orig_idx]));
                            if !b.orig[orig_idx].ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                    similar::DiffOp::Replace {
                        new_index, new_len, ..
                    } => {
                        for i in 0..*new_len {
                            let orig_idx = b.map[new_index + i];
                            output.push_str(&format!("! {}", b.orig[orig_idx]));
                            if !b.orig[orig_idx].ends_with('\n') {
                                output.push('\n');
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    output
}

fn diff_files(
    path1: &str,
    path2: &str,
    content1: &str,
    content2: &str,
    opts: &DiffOpts,
    stdout: &mut String,
) -> i32 {
    let input_a = preprocess_lines(content1, opts);
    let input_b = preprocess_lines(content2, opts);

    if input_a.proc == input_b.proc {
        if opts.report_identical {
            stdout.push_str(&format!("Files {} and {} are identical\n", path1, path2));
        }
        return 0;
    }

    if opts.brief {
        stdout.push_str(&format!("Files {} and {} differ\n", path1, path2));
        return 1;
    }

    let label1 = if !opts.labels.is_empty() {
        opts.labels[0].to_string()
    } else {
        path1.to_string()
    };
    let label2 = if opts.labels.len() > 1 {
        opts.labels[1].to_string()
    } else {
        path2.to_string()
    };

    let diff_output = match opts.format {
        OutputFormat::Normal => format_normal_diff(&input_a, &input_b),
        OutputFormat::Unified(ctx) => {
            format_unified_diff(&input_a, &input_b, &label1, &label2, ctx)
        }
        OutputFormat::Context(ctx) => {
            format_context_diff(&input_a, &input_b, &label1, &label2, ctx)
        }
    };

    stdout.push_str(&diff_output);
    1
}

fn diff_directories(
    path1: &str,
    path2: &str,
    opts: &DiffOpts,
    ctx: &CommandContext,
    stdout: &mut String,
    stderr: &mut String,
) -> i32 {
    let resolved1 = resolve_path(path1, ctx.cwd);
    let resolved2 = resolve_path(path2, ctx.cwd);

    let entries1 = match ctx.fs.readdir(&resolved1) {
        Ok(entries) => entries,
        Err(e) => {
            stderr.push_str(&format!("diff: {}: {}\n", path1, e));
            return 2;
        }
    };

    let entries2 = match ctx.fs.readdir(&resolved2) {
        Ok(entries) => entries,
        Err(e) => {
            stderr.push_str(&format!("diff: {}: {}\n", path2, e));
            return 2;
        }
    };

    let names1: std::collections::BTreeSet<String> =
        entries1.iter().map(|e| e.name.clone()).collect();
    let names2: std::collections::BTreeSet<String> =
        entries2.iter().map(|e| e.name.clone()).collect();
    let all_names: std::collections::BTreeSet<String> = names1.union(&names2).cloned().collect();

    let mut exit_code = 0;

    for name in &all_names {
        let child1 = format!("{}/{}", path1, name);
        let child2 = format!("{}/{}", path2, name);
        let in1 = names1.contains(name);
        let in2 = names2.contains(name);

        if in1 && !in2 {
            stdout.push_str(&format!("Only in {}: {}\n", path1, name));
            if exit_code < 1 {
                exit_code = 1;
            }
            continue;
        }
        if !in1 && in2 {
            stdout.push_str(&format!("Only in {}: {}\n", path2, name));
            if exit_code < 1 {
                exit_code = 1;
            }
            continue;
        }

        // Both exist
        let is_dir1 = is_directory(&child1, ctx);
        let is_dir2 = is_directory(&child2, ctx);

        if is_dir1 && is_dir2 {
            let code = diff_directories(&child1, &child2, opts, ctx, stdout, stderr);
            if code > exit_code {
                exit_code = code;
            }
        } else if !is_dir1 && !is_dir2 {
            let content1 = match read_file_content(&child1, ctx) {
                Ok(c) => c,
                Err(e) => {
                    stderr.push_str(&format!("{}\n", e));
                    exit_code = 2;
                    continue;
                }
            };
            let content2 = match read_file_content(&child2, ctx) {
                Ok(c) => c,
                Err(e) => {
                    stderr.push_str(&format!("{}\n", e));
                    exit_code = 2;
                    continue;
                }
            };

            let code = diff_files(&child1, &child2, &content1, &content2, opts, stdout);
            if code > exit_code {
                exit_code = code;
            }
        } else {
            stdout.push_str(&format!(
                "File {} is a {} while file {} is a {}\n",
                child1,
                if is_dir1 { "directory" } else { "regular file" },
                child2,
                if is_dir2 { "directory" } else { "regular file" },
            ));
            if exit_code < 1 {
                exit_code = 1;
            }
        }
    }

    exit_code
}

fn parse_args<'a>(args: &'a [String]) -> Result<(DiffOpts<'a>, Vec<&'a str>), String> {
    let mut opts = DiffOpts::default();
    let mut files: Vec<&str> = Vec::new();
    let mut opts_done = false;
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];

        if opts_done || !arg.starts_with('-') || arg == "-" {
            files.push(arg);
            i += 1;
            continue;
        }

        if arg == "--" {
            opts_done = true;
            i += 1;
            continue;
        }

        // Long options
        if arg.starts_with("--") {
            match arg.as_str() {
                "--unified" => {
                    opts.format = OutputFormat::Unified(3);
                }
                "--context" => {
                    opts.format = OutputFormat::Context(3);
                }
                "--recursive" => opts.recursive = true,
                "--brief" => opts.brief = true,
                "--report-identical-files" => opts.report_identical = true,
                "--new-file" => opts.new_file = true,
                "--ignore-case" => opts.ignore_case = true,
                "--ignore-all-space" => opts.ignore_all_space = true,
                "--ignore-space-change" => opts.ignore_space_change = true,
                "--ignore-blank-lines" => opts.ignore_blank_lines = true,
                "--label" => {
                    i += 1;
                    if i >= args.len() {
                        return Err("diff: option '--label' requires an argument".to_string());
                    }
                    opts.labels.push(&args[i]);
                }
                _ if arg.starts_with("--unified=") => {
                    let val = &arg["--unified=".len()..];
                    let n: usize = val
                        .parse()
                        .map_err(|_| format!("diff: invalid context length '{}'", val))?;
                    opts.format = OutputFormat::Unified(n);
                }
                _ if arg.starts_with("--context=") => {
                    let val = &arg["--context=".len()..];
                    let n: usize = val
                        .parse()
                        .map_err(|_| format!("diff: invalid context length '{}'", val))?;
                    opts.format = OutputFormat::Context(n);
                }
                _ if arg.starts_with("--label=") => {
                    let val = &arg["--label=".len()..];
                    opts.labels.push(val);
                }
                _ => {
                    return Err(format!("diff: unrecognized option '{}'", arg));
                }
            }
            i += 1;
            continue;
        }

        // Short options
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut j = 0;
        while j < chars.len() {
            match chars[j] {
                'u' => opts.format = OutputFormat::Unified(3),
                'c' => opts.format = OutputFormat::Context(3),
                'r' => opts.recursive = true,
                'q' => opts.brief = true,
                's' => opts.report_identical = true,
                'N' => opts.new_file = true,
                'i' => opts.ignore_case = true,
                'w' => opts.ignore_all_space = true,
                'b' => opts.ignore_space_change = true,
                'B' => opts.ignore_blank_lines = true,
                'U' => {
                    let rest: String = chars[j + 1..].iter().collect();
                    let val = if !rest.is_empty() {
                        rest
                    } else {
                        i += 1;
                        if i >= args.len() {
                            return Err("diff: option requires an argument -- 'U'".to_string());
                        }
                        args[i].clone()
                    };
                    let n: usize = val
                        .parse()
                        .map_err(|_| format!("diff: invalid context length '{}'", val))?;
                    opts.format = OutputFormat::Unified(n);
                    j = chars.len(); // consumed the rest
                    continue;
                }
                'C' => {
                    let rest: String = chars[j + 1..].iter().collect();
                    let val = if !rest.is_empty() {
                        rest
                    } else {
                        i += 1;
                        if i >= args.len() {
                            return Err("diff: option requires an argument -- 'C'".to_string());
                        }
                        args[i].clone()
                    };
                    let n: usize = val
                        .parse()
                        .map_err(|_| format!("diff: invalid context length '{}'", val))?;
                    opts.format = OutputFormat::Context(n);
                    j = chars.len();
                    continue;
                }
                _ => {
                    return Err(format!("diff: invalid option -- '{}'", chars[j]));
                }
            }
            j += 1;
        }
        i += 1;
    }

    Ok((opts, files))
}

static DIFF_META: CommandMeta = CommandMeta {
    name: "diff",
    synopsis: "diff [OPTIONS] FILE1 FILE2",
    description: "Compare files line by line.",
    options: &[
        ("-u, --unified", "output in unified format"),
        ("-c, --context", "output in context format"),
        ("-r, --recursive", "recursively compare directories"),
        ("-q, --brief", "report only when files differ"),
        ("-s, --report-identical", "report when files are identical"),
        ("-N, --new-file", "treat absent files as empty"),
        ("-i, --ignore-case", "ignore case differences"),
        ("-w, --ignore-all-space", "ignore all white space"),
        (
            "-b, --ignore-space-change",
            "ignore changes in amount of white space",
        ),
        (
            "-B, --ignore-blank-lines",
            "ignore changes where lines are all blank",
        ),
        ("--label LABEL", "use LABEL instead of file name"),
    ],
    supports_help_flag: true,
};

impl super::VirtualCommand for DiffCommand {
    fn name(&self) -> &str {
        "diff"
    }

    fn meta(&self) -> Option<&'static CommandMeta> {
        Some(&DIFF_META)
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let (opts, files) = match parse_args(args) {
            Ok(v) => v,
            Err(e) => {
                return CommandResult {
                    stderr: format!("{}\n", e),
                    exit_code: 2,
                    ..Default::default()
                };
            }
        };

        if files.len() != 2 {
            return CommandResult {
                stderr: "diff: requires exactly two file arguments\n".to_string(),
                exit_code: 2,
                ..Default::default()
            };
        }

        let path1 = files[0];
        let path2 = files[1];

        let dir1 = is_directory(path1, ctx);
        let dir2 = is_directory(path2, ctx);

        // Recursive directory diff
        if dir1 && dir2 {
            if !opts.recursive {
                return CommandResult {
                    stderr: format!(
                        "diff: {} is a directory\ndiff: {} is a directory\n",
                        path1, path2
                    ),
                    exit_code: 2,
                    ..Default::default()
                };
            }
            let mut stdout = String::new();
            let mut stderr = String::new();
            let exit_code = diff_directories(path1, path2, &opts, ctx, &mut stdout, &mut stderr);
            return CommandResult {
                stdout,
                stderr,
                exit_code,
            };
        }

        // If one is a directory and the other is a file, diff the file against same-named file in dir
        if dir1 || dir2 {
            let (dir_path, file_path) = if dir1 { (path1, path2) } else { (path2, path1) };
            let filename = std::path::Path::new(file_path)
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| file_path.to_string());
            let dir_file = format!("{}/{}", dir_path, filename);

            let content_file = match read_file_content(file_path, ctx) {
                Ok(c) => c,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("{}\n", e),
                        exit_code: 2,
                        ..Default::default()
                    };
                }
            };
            let content_dir = match read_file_content(&dir_file, ctx) {
                Ok(c) => c,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("{}\n", e),
                        exit_code: 2,
                        ..Default::default()
                    };
                }
            };

            let (c1, c2, p1, p2) = if dir1 {
                (content_dir, content_file, dir_file.as_str(), file_path)
            } else {
                (content_file, content_dir, file_path, dir_file.as_str())
            };

            let mut stdout = String::new();
            let exit_code = diff_files(p1, p2, &c1, &c2, &opts, &mut stdout);
            return CommandResult {
                stdout,
                exit_code,
                ..Default::default()
            };
        }

        // Handle -N (treat absent files as empty)
        let exists1 = file_exists(path1, ctx);
        let exists2 = file_exists(path2, ctx);

        if !exists1 && !opts.new_file {
            return CommandResult {
                stderr: format!("diff: {}: No such file or directory\n", path1),
                exit_code: 2,
                ..Default::default()
            };
        }
        if !exists2 && !opts.new_file {
            return CommandResult {
                stderr: format!("diff: {}: No such file or directory\n", path2),
                exit_code: 2,
                ..Default::default()
            };
        }

        let content1 = if exists1 {
            match read_file_content(path1, ctx) {
                Ok(c) => c,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("{}\n", e),
                        exit_code: 2,
                        ..Default::default()
                    };
                }
            }
        } else {
            String::new()
        };

        let content2 = if exists2 {
            match read_file_content(path2, ctx) {
                Ok(c) => c,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("{}\n", e),
                        exit_code: 2,
                        ..Default::default()
                    };
                }
            }
        } else {
            String::new()
        };

        let mut stdout = String::new();
        let exit_code = diff_files(path1, path2, &content1, &content2, &opts, &mut stdout);

        CommandResult {
            stdout,
            exit_code,
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

    fn ctx_with_stdin<'a>(
        fs: &'a Arc<InMemoryFs>,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        network_policy: &'a NetworkPolicy,
        stdin: &'a str,
    ) -> CommandContext<'a> {
        CommandContext {
            fs: &**fs,
            cwd: "/",
            env,
            stdin,
            limits,
            network_policy,
            exec: None,
        }
    }

    fn ctx<'a>(
        fs: &'a Arc<InMemoryFs>,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        network_policy: &'a NetworkPolicy,
    ) -> CommandContext<'a> {
        ctx_with_stdin(fs, env, limits, network_policy, "")
    }

    fn run(args: &[&str], context: &CommandContext) -> CommandResult {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        DiffCommand.execute(&owned, context)
    }

    #[test]
    fn identical_files_exit_zero_no_output() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\nworld\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello\nworld\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn different_files_exit_one_normal_diff() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"world\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("1c1"));
        assert!(result.stdout.contains("< hello"));
        assert!(result.stdout.contains("> world"));
    }

    #[test]
    fn unified_format_headers_and_hunks() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"line1\nline2\nline3\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"line1\nchanged\nline3\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-u", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("--- /a.txt"));
        assert!(result.stdout.contains("+++ /b.txt"));
        assert!(result.stdout.contains("@@"));
        assert!(result.stdout.contains("-line2"));
        assert!(result.stdout.contains("+changed"));
    }

    #[test]
    fn context_format_headers() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"line1\nline2\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"line1\nline3\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-c", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("*** /a.txt"));
        assert!(result.stdout.contains("--- /b.txt"));
        assert!(result.stdout.contains("***************"));
    }

    #[test]
    fn recursive_directory_comparison() {
        let (fs, env, limits, np) = test_ctx();
        fs.mkdir_p(std::path::Path::new("/dir1")).unwrap();
        fs.mkdir_p(std::path::Path::new("/dir2")).unwrap();
        fs.write_file(std::path::Path::new("/dir1/a.txt"), b"hello\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/dir2/a.txt"), b"world\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/dir1/b.txt"), b"same\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/dir2/b.txt"), b"same\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/dir1/only1.txt"), b"x\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/dir2/only2.txt"), b"y\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-r", "/dir1", "/dir2"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("Only in /dir1: only1.txt"));
        assert!(result.stdout.contains("Only in /dir2: only2.txt"));
        // a.txt differs
        assert!(result.stdout.contains("1c1"));
    }

    #[test]
    fn brief_mode() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"world\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-q", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert_eq!(result.stdout, "Files /a.txt and /b.txt differ\n");
    }

    #[test]
    fn ignore_case() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"Hello\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-i", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn ignore_all_whitespace() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello world\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"helloworld\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-w", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn ignore_space_change() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello  world\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello world\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-b", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn new_file_treats_absent_as_empty() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-N", "/a.txt", "/nonexistent.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("< hello"));
    }

    #[test]
    fn absent_file_without_new_file_flag_errors() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/nonexistent.txt"], &c);
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("No such file or directory"));
    }

    #[test]
    fn report_identical_files() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"same\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"same\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-s", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "Files /a.txt and /b.txt are identical\n");
    }

    #[test]
    fn single_line_files() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"a").unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"b").unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("< a"));
        assert!(result.stdout.contains("> b"));
    }

    #[test]
    fn empty_files_identical() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"").unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"").unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn stdin_via_dash() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/b.txt"), b"world\n")
            .unwrap();
        let c = ctx_with_stdin(&fs, &env, &limits, &np, "hello\n");
        let result = run(&["-", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("< hello"));
        assert!(result.stdout.contains("> world"));
    }

    #[test]
    fn unified_custom_context() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(
            std::path::Path::new("/a.txt"),
            b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n",
        )
        .unwrap();
        fs.write_file(
            std::path::Path::new("/b.txt"),
            b"1\n2\n3\n4\nFIVE\n6\n7\n8\n9\n10\n",
        )
        .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-U1", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("--- /a.txt"));
        assert!(result.stdout.contains("+++ /b.txt"));
        assert!(result.stdout.contains("-5"));
        assert!(result.stdout.contains("+FIVE"));
    }

    #[test]
    fn label_overrides_filename() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"world\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(
            &["-u", "--label", "old", "--label", "new", "/a.txt", "/b.txt"],
            &c,
        );
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("--- old"));
        assert!(result.stdout.contains("+++ new"));
    }

    #[test]
    fn no_trailing_newline_files() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn ignore_blank_lines() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\n\nworld\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello\nworld\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-B", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn new_file_absent_first_file() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-N", "/nonexistent.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("> hello"));
    }

    #[test]
    fn directories_without_recursive_flag_errors() {
        let (fs, env, limits, np) = test_ctx();
        fs.mkdir_p(std::path::Path::new("/dir1")).unwrap();
        fs.mkdir_p(std::path::Path::new("/dir2")).unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/dir1", "/dir2"], &c);
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("is a directory"));
    }

    #[test]
    fn requires_two_arguments() {
        let (fs, env, limits, np) = test_ctx();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt"], &c);
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("requires exactly two"));
    }

    #[test]
    fn normal_diff_add_lines() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"line1\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"line1\nline2\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("1a2"));
        assert!(result.stdout.contains("> line2"));
    }

    #[test]
    fn normal_diff_delete_lines() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"line1\nline2\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"line1\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("2d1"));
        assert!(result.stdout.contains("< line2"));
    }

    #[test]
    fn recursive_nested_directories() {
        let (fs, env, limits, np) = test_ctx();
        fs.mkdir_p(std::path::Path::new("/d1/sub")).unwrap();
        fs.mkdir_p(std::path::Path::new("/d2/sub")).unwrap();
        fs.write_file(std::path::Path::new("/d1/sub/f.txt"), b"old\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/d2/sub/f.txt"), b"new\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-r", "/d1", "/d2"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("< old"));
        assert!(result.stdout.contains("> new"));
    }

    #[test]
    fn context_format_custom_context() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"1\n2\n3\n4\n5\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"1\n2\nX\n4\n5\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-C1", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("*** /a.txt"));
        assert!(result.stdout.contains("--- /b.txt"));
        assert!(result.stdout.contains("***************"));
    }

    #[test]
    fn new_file_flag_with_unified() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"hello\nworld\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-uN", "/a.txt", "/missing.txt"], &c);
        assert_eq!(result.exit_code, 1);
        assert!(result.stdout.contains("--- /a.txt"));
        assert!(result.stdout.contains("+++ /missing.txt"));
    }

    #[test]
    fn brief_identical_no_output() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"same\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"same\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-q", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout, "");
    }

    #[test]
    fn combined_flags() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"HELLO  WORLD\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello world\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        // -i ignores case, -b ignores space changes → should be identical
        let result = run(&["-ib", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 0);
    }

    #[test]
    fn ignore_case_affects_diff_output() {
        // Verify -i flag influences the diff algorithm, not just identity check.
        // Lines that differ only in case should be treated as equal context,
        // so only the truly different line appears in output.
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"Hello\nfoo\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"hello\nbar\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-i", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        // "Hello" vs "hello" should match with -i, so only foo/bar differs
        assert!(result.stdout.contains("< foo"));
        assert!(result.stdout.contains("> bar"));
        // Should NOT report a change on the Hello/hello line
        assert!(!result.stdout.contains("< Hello"));
    }

    #[test]
    fn context_format_uses_exclamation_for_replace() {
        let (fs, env, limits, np) = test_ctx();
        fs.write_file(std::path::Path::new("/a.txt"), b"line1\nold\nline3\n")
            .unwrap();
        fs.write_file(std::path::Path::new("/b.txt"), b"line1\nnew\nline3\n")
            .unwrap();
        let c = ctx(&fs, &env, &limits, &np);
        let result = run(&["-c", "/a.txt", "/b.txt"], &c);
        assert_eq!(result.exit_code, 1);
        // Replace ops should use ! marker, not - / +
        assert!(result.stdout.contains("! old"));
        assert!(result.stdout.contains("! new"));
    }
}
