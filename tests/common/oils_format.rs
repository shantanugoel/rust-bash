// Parser for the Oils spec test `.test.sh` format.
// Format reference: <https://www.oilshell.org/cross-ref.html#spec-test>

/// A single test case parsed from an Oils spec test file.
pub struct OilsTestCase {
    pub name: String,
    pub code: String,
    pub expected_stdout: Option<String>,
    pub expected_stderr: Option<String>,
    pub expected_status: i32,
}

/// An entire Oils spec test file.
pub struct OilsTestFile {
    pub cases: Vec<OilsTestCase>,
    pub tags: Vec<String>,
}

/// Parse an Oils `.test.sh` file into an `OilsTestFile`.
pub fn parse_oils_file(content: &str) -> OilsTestFile {
    let lines: Vec<&str> = content.lines().collect();
    let mut tags: Vec<String> = Vec::new();
    let mut cases: Vec<OilsTestCase> = Vec::new();

    // Find where test cases begin (first `#### ` line).
    let first_case_idx = lines
        .iter()
        .position(|l| l.starts_with("#### "))
        .unwrap_or(lines.len());

    // Parse file-level headers (before any test case).
    for line in &lines[..first_case_idx] {
        if let Some(rest) = line.strip_prefix("## tags: ") {
            tags.extend(rest.split_whitespace().map(String::from));
        }
        // Other file-level annotations (compare_shells, oils_failures_allowed, etc.) are ignored.
    }

    // Split into per-case chunks delimited by `#### `.
    let mut case_starts: Vec<usize> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if line.starts_with("#### ") {
            case_starts.push(i);
        }
    }

    for (ci, &start) in case_starts.iter().enumerate() {
        let end = if ci + 1 < case_starts.len() {
            case_starts[ci + 1]
        } else {
            lines.len()
        };
        let case = parse_case(&lines[start..end]);
        cases.push(case);
    }

    OilsTestFile { cases, tags }
}

/// Parse a single test case from the lines between two `#### ` delimiters.
fn parse_case(lines: &[&str]) -> OilsTestCase {
    let name = lines[0]
        .strip_prefix("#### ")
        .unwrap_or("")
        .trim()
        .to_string();

    // Separate code lines from metadata lines.
    // In the Oils format, `## ` at the start of a line is always metadata — shell code
    // never uses `## ` (shell comments use single `#`).
    // Metadata starts at the first `## ` line after the header.
    let body = &lines[1..];

    let meta_start = body
        .iter()
        .position(|l| l.starts_with("## "))
        .unwrap_or(body.len());
    let mut code_lines: Vec<&str> = body[..meta_start].to_vec();

    // Trim trailing blank lines from code.
    while code_lines.last().is_some_and(|l| l.is_empty()) {
        code_lines.pop();
    }

    let mut code = code_lines.join("\n");

    // Parse metadata lines.
    let meta_lines = if meta_start < body.len() {
        &body[meta_start..]
    } else {
        &[] as &[&str]
    };

    let mut default_stdout: Option<String> = None;
    let mut default_stderr: Option<String> = None;
    let mut default_status: Option<i32> = None;

    // Bash-specific overrides (N-I, BUG, OK).
    let mut ni_stdout: Option<String> = None;
    let mut ni_stderr: Option<String> = None;
    let mut ni_status: Option<i32> = None;
    let mut bug_stdout: Option<String> = None;
    let mut bug_stderr: Option<String> = None;
    let mut bug_status: Option<i32> = None;
    let mut ok_stdout: Option<String> = None;
    let mut ok_stderr: Option<String> = None;
    let mut ok_status: Option<i32> = None;

    let mut i = 0;
    while i < meta_lines.len() {
        let line = meta_lines[i];
        let stripped = match line.strip_prefix("## ") {
            Some(s) => s,
            None => {
                i += 1;
                continue;
            }
        };

        // Try to match bash-specific override patterns first.
        if let Some((kind, rest)) = parse_bash_override_prefix(stripped) {
            let (stdout_slot, stderr_slot, status_slot) = match kind {
                OverrideKind::NI => (&mut ni_stdout, &mut ni_stderr, &mut ni_status),
                OverrideKind::Bug => (&mut bug_stdout, &mut bug_stderr, &mut bug_status),
                OverrideKind::Ok => (&mut ok_stdout, &mut ok_stderr, &mut ok_status),
            };

            if let Some(val) = rest.strip_prefix("stdout: ") {
                *stdout_slot = Some(ensure_trailing_newline_if_nonempty(val));
            } else if rest == "stdout:" {
                *stdout_slot = Some(String::new());
            } else if rest == "STDOUT:" {
                let (block, consumed) = parse_multiline_block(&meta_lines[i + 1..]);
                *stdout_slot = Some(block);
                i += consumed;
            } else if let Some(val) = rest.strip_prefix("stdout-json: ") {
                *stdout_slot = decode_json_string(val);
            } else if let Some(val) = rest.strip_prefix("stderr: ") {
                *stderr_slot = Some(ensure_trailing_newline_if_nonempty(val));
            } else if rest == "stderr:" {
                *stderr_slot = Some(String::new());
            } else if rest == "STDERR:" {
                let (block, consumed) = parse_multiline_block(&meta_lines[i + 1..]);
                *stderr_slot = Some(block);
                i += consumed;
            } else if let Some(val) = rest.strip_prefix("status: ") {
                if let Ok(n) = val.trim().parse::<i32>() {
                    *status_slot = Some(n);
                }
            } else if let Some(val) = rest.strip_prefix("stderr-json: ") {
                *stderr_slot = decode_json_string(val);
            }
            i += 1;
            continue;
        }

        // Default (non-override) metadata.
        if let Some(val) = stripped.strip_prefix("code: ") {
            code = val.to_string();
        } else if let Some(val) = stripped.strip_prefix("stdout: ") {
            default_stdout = Some(ensure_trailing_newline_if_nonempty(val));
        } else if stripped == "stdout:" {
            default_stdout = Some(String::new());
        } else if stripped == "STDOUT:" {
            let (block, consumed) = parse_multiline_block(&meta_lines[i + 1..]);
            default_stdout = Some(block);
            i += consumed;
        } else if let Some(val) = stripped.strip_prefix("stdout-json: ") {
            default_stdout = decode_json_string(val);
        } else if let Some(val) = stripped.strip_prefix("stderr: ") {
            default_stderr = Some(ensure_trailing_newline_if_nonempty(val));
        } else if stripped == "stderr:" {
            default_stderr = Some(String::new());
        } else if stripped == "STDERR:" {
            let (block, consumed) = parse_multiline_block(&meta_lines[i + 1..]);
            default_stderr = Some(block);
            i += consumed;
        } else if let Some(val) = stripped.strip_prefix("status: ") {
            if let Ok(n) = val.trim().parse::<i32>() {
                default_status = Some(n);
            }
        } else if let Some(val) = stripped.strip_prefix("stderr-json: ") {
            default_stderr = decode_json_string(val);
        }
        // Unrecognized `## ` lines are ignored (comments, tags, etc.).

        i += 1;
    }

    // Apply override priority: N-I > BUG > OK > default.
    // Trailing-newline normalization is already applied at storage time for inline
    // annotations. JSON-decoded and multiline-block values are stored verbatim.
    let expected_stdout = ni_stdout.or(bug_stdout).or(ok_stdout).or(default_stdout);
    let expected_stderr = ni_stderr.or(bug_stderr).or(ok_stderr).or(default_stderr);
    let expected_status = ni_status
        .or(bug_status)
        .or(ok_status)
        .or(default_status)
        .unwrap_or(0);

    OilsTestCase {
        name,
        code,
        expected_stdout,
        expected_stderr,
        expected_status,
    }
}

/// Inline stdout values in Oils format don't include a trailing newline in the annotation,
/// but the actual shell output will have one. Add it if the string is non-empty.
fn ensure_trailing_newline_if_nonempty(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    if s.ends_with('\n') {
        s.to_string()
    } else {
        format!("{s}\n")
    }
}

#[derive(Debug, Clone, Copy)]
enum OverrideKind {
    NI,
    Bug,
    Ok,
}

/// Try to parse a bash-specific override prefix from a metadata line (after `## `).
/// Returns the override kind and the remaining metadata key-value string if bash is in the
/// shell list.
///
/// Examples:
///   "N-I bash stdout: foo"       -> Some((NI, "stdout: foo"))
///   "BUG bash dash stdout: bar"  -> Some((Bug, "stdout: bar"))
///   "OK zsh stdout: baz"         -> None (bash not in list)
fn parse_bash_override_prefix(s: &str) -> Option<(OverrideKind, &str)> {
    let (kind, rest) = if let Some(r) = s.strip_prefix("N-I ") {
        (OverrideKind::NI, r)
    } else if let Some(r) = s.strip_prefix("BUG ") {
        (OverrideKind::Bug, r)
    } else if let Some(r) = s.strip_prefix("OK ") {
        (OverrideKind::Ok, r)
    } else {
        return None;
    };

    // The rest is: shell_list metadata_key_value
    // Shell list tokens are space-separated and end when we hit a known metadata keyword.
    let metadata_keywords = [
        "stdout:",
        "stdout-json:",
        "STDOUT:",
        "stderr:",
        "STDERR:",
        "status:",
        "stderr-json:",
    ];

    let mut found_bash = false;
    let mut keyword_start = None;

    for (i, token) in rest.split_whitespace().enumerate() {
        if metadata_keywords.iter().any(|kw| token.starts_with(kw)) {
            // This token is the start of the metadata. Everything before is shell names.
            // Calculate byte offset.
            let byte_offset = byte_offset_of_nth_token(rest, i);
            keyword_start = Some(byte_offset);
            break;
        }
        if token.split('/').any(|s| s == "bash") {
            found_bash = true;
        }
    }

    if found_bash && let Some(offset) = keyword_start {
        return Some((kind, &rest[offset..]));
    }

    None
}

/// Find the byte offset of the nth whitespace-separated token in `s`.
fn byte_offset_of_nth_token(s: &str, n: usize) -> usize {
    let mut count = 0;
    let mut in_token = false;
    for (i, ch) in s.char_indices() {
        if ch.is_whitespace() {
            in_token = false;
        } else if !in_token {
            if count == n {
                return i;
            }
            count += 1;
            in_token = true;
        }
    }
    s.len()
}

/// Parse a multiline `## STDOUT:` / `## STDERR:` block.
/// Reads lines until `## END` and returns the collected text plus the number of lines consumed
/// (including the `## END` line).  A `## ` line that is *not* `## END` also terminates the block
/// (without being consumed) — this handles adjacent override blocks such as
/// `## STDOUT: ... ## N-I mksh STDOUT: ... ## END`.
fn parse_multiline_block(lines: &[&str]) -> (String, usize) {
    let mut result = String::new();
    let mut consumed = 0;
    let mut line_count = 0;
    for line in lines {
        if *line == "## END" {
            consumed += 1; // consume the terminator
            break;
        }
        if line.starts_with("## ") {
            // Another metadata line starts a new section — don't consume it.
            break;
        }
        consumed += 1;
        if line_count > 0 {
            result.push('\n');
        }
        result.push_str(line);
        line_count += 1;
    }
    if line_count > 0 {
        result.push('\n');
    }
    (result, consumed)
}

/// Decode a JSON-encoded string value like `"hello\n"`.
fn decode_json_string(s: &str) -> Option<String> {
    let trimmed = s.trim();
    serde_json::from_str::<String>(trimmed).ok()
}

/// Validate the parser with a suite of unit-level checks.
/// Called from the oils_spec test harness since `#[test]` is unavailable in `harness = false` binaries.
pub fn run_parser_unit_tests() {
    // Simple case
    {
        let input = "## tags: dev-minimal\n\n#### echo hello\necho hello\n## stdout: hello\n";
        let file = parse_oils_file(input);
        assert_eq!(file.tags, vec!["dev-minimal"]);
        assert_eq!(file.cases.len(), 1);
        assert_eq!(file.cases[0].name, "echo hello");
        assert_eq!(file.cases[0].code, "echo hello");
        assert_eq!(file.cases[0].expected_stdout, Some("hello\n".to_string()));
        assert_eq!(file.cases[0].expected_status, 0);
    }

    // Status
    {
        let input = "#### failing\nfalse\n## status: 1\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_status, 1);
    }

    // Multiline stdout
    {
        let input = "#### multi\necho a; echo b\n## STDOUT:\na\nb\n## END\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("a\nb\n".to_string()));
    }

    // N-I bash override
    {
        let input = "#### test\necho hi\n## stdout: hi\n## N-I bash stdout: nope\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("nope\n".to_string()));
    }

    // BUG bash override
    {
        let input = "#### test\necho hi\n## stdout: hi\n## BUG bash stdout: buggy\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("buggy\n".to_string()));
    }

    // OK bash override
    {
        let input = "#### test\necho hi\n## stdout: hi\n## OK bash stdout: ok_val\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("ok_val\n".to_string()));
    }

    // N-I > BUG priority
    {
        let input = "#### test\necho hi\n## stdout: hi\n## BUG bash stdout: buggy\n## N-I bash stdout: nope\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("nope\n".to_string()));
    }

    // Non-bash override ignored
    {
        let input = "#### test\necho hi\n## stdout: hi\n## N-I zsh stdout: nope\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("hi\n".to_string()));
    }

    // Multi-shell override with bash in list
    {
        let input = "#### test\necho hi\n## stdout: hi\n## N-I bash dash stdout: nope\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("nope\n".to_string()));
    }

    // Multiple cases
    {
        let input = "#### first\necho a\n## stdout: a\n\n#### second\necho b\n## stdout: b\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases.len(), 2);
        assert_eq!(file.cases[0].name, "first");
        assert_eq!(file.cases[1].name, "second");
    }

    // Empty stdout
    {
        let input = "#### test\ntrue\n## stdout:\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some(String::new()));
    }

    // stderr-json
    {
        let input = "#### test\necho err >&2\n## stderr-json: \"err\\n\"\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stderr, Some("err\n".to_string()));
    }

    // N-I bash status override
    {
        let input = "#### test\nfalse\n## status: 0\n## N-I bash status: 1\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_status, 1);
    }

    // N-I bash multiline stdout override
    {
        let input = "#### test\necho hi\n## STDOUT:\ndefault\n## END\n## N-I bash STDOUT:\noverride\n## END\n";
        let file = parse_oils_file(input);
        assert_eq!(
            file.cases[0].expected_stdout,
            Some("override\n".to_string())
        );
    }

    // stdout-json (Finding 1 fix)
    {
        let input = "#### test\necho hi\n## stdout-json: \"hello\\n\"\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("hello\n".to_string()));
    }

    // stdout-json empty string
    {
        let input = "#### test\ntrue\n## stdout-json: \"\"\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some(String::new()));
    }

    // Slash-separated shell list (Finding 2 fix)
    {
        let input =
            "#### test\necho hi\n## stdout: default\n## N-I dash/bash/mksh stdout: overridden\n";
        let file = parse_oils_file(input);
        assert_eq!(
            file.cases[0].expected_stdout,
            Some("overridden\n".to_string())
        );
    }

    // JSON-decoded values should NOT get extra newline (Finding 3 fix)
    {
        let input = "#### test\necho err >&2\n## stderr-json: \"err\"\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stderr, Some("err".to_string()));
    }

    // N-I bash stdout-json override
    {
        let input =
            "#### test\necho hi\n## stdout: default\n## N-I bash stdout-json: \"nope\\n\"\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("nope\n".to_string()));
    }

    // Multiline STDOUT block terminated by override STDOUT block (parser bug fix)
    {
        let input =
            "#### test\necho hi\n## STDOUT:\n1\n1\n0\n## N-I mksh STDOUT:\n127\n127\n127\n## END\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("1\n1\n0\n".to_string()));
    }

    // STDOUT block with empty first line
    {
        let input = "#### test\necho; echo 5\n## STDOUT:\n\n5\n## END\n";
        let file = parse_oils_file(input);
        assert_eq!(file.cases[0].expected_stdout, Some("\n5\n".to_string()));
    }

    eprintln!("--- oils_format parser: all 22 unit tests passed");
}
