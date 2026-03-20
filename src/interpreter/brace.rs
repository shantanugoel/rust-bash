//! Brace expansion: `{a,b,c}` alternation and `{1..10}` sequence expansion.
//!
//! Brace expansion is a purely textual transformation that happens BEFORE all
//! other expansions (variable, command substitution, glob). It operates on the
//! raw word string and produces a list of expanded strings.

use crate::error::RustBashError;

/// Expand brace expressions in a raw word string.
///
/// Returns a list of expanded strings. If no brace expansion applies, returns
/// a single-element list containing the original string.
///
/// `max_results` caps total expansion output to prevent unbounded growth.
pub fn brace_expand(input: &str, max_results: usize) -> Result<Vec<String>, RustBashError> {
    expand_recursive(input, max_results)
}

/// Recursively expand brace expressions in the input string.
fn expand_recursive(input: &str, max_results: usize) -> Result<Vec<String>, RustBashError> {
    let Some((open, close)) = find_brace_pair(input) else {
        return Ok(vec![input.to_string()]);
    };

    let prefix = &input[..open];
    let body = &input[open + 1..close];
    let suffix = &input[close + 1..];

    // Try sequence expansion first: {a..b} or {a..b..c}
    if let Some(seq) = try_sequence_expansion(body) {
        let mut results = Vec::new();
        for item in &seq {
            let expanded_suffixes = expand_recursive(suffix, max_results)?;
            for s in &expanded_suffixes {
                results.push(format!("{prefix}{item}{s}"));
                check_limit(results.len(), max_results)?;
            }
        }
        return Ok(results);
    }

    // Comma-separated alternation: {a,b,c}
    let alternatives = split_alternatives(body);

    // Single item → no expansion (literal braces)
    if alternatives.len() < 2 {
        return Ok(vec![input.to_string()]);
    }

    let mut results = Vec::new();
    for alt in &alternatives {
        let combined = format!("{prefix}{alt}{suffix}");
        let expanded = expand_recursive(&combined, max_results)?;
        for item in expanded {
            results.push(item);
            check_limit(results.len(), max_results)?;
        }
    }

    Ok(results)
}

fn check_limit(count: usize, max: usize) -> Result<(), RustBashError> {
    if count >= max {
        return Err(RustBashError::LimitExceeded(format!(
            "brace expansion exceeded limit of {max} results"
        )));
    }
    Ok(())
}

// ── Byte-level scanner helpers ──────────────────────────────────────
//
// All scanning works on bytes. Since the delimiter characters we care about
// (`{`, `}`, `,`, `$`, `\\`, `'`, `"`, `(`, `)`) are all ASCII, and UTF-8
// continuation bytes (0x80–0xBF) never collide with ASCII, byte-level
// scanning is safe. Content is always extracted by slicing the original `&str`
// to preserve multi-byte characters correctly.

/// Advance past a `${...}` parameter expansion starting at `bytes[i]` == `$`.
fn skip_dollar_brace(bytes: &[u8], start: usize) -> usize {
    debug_assert!(bytes[start] == b'$' && bytes.get(start + 1) == Some(&b'{'));
    let len = bytes.len();
    let mut i = start + 2;
    let mut depth = 1;
    while i < len && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b'\\' if i + 1 < len => {
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// Advance past a `$(...)` command substitution (or `$((..))` arithmetic).
fn skip_dollar_paren(bytes: &[u8], start: usize) -> usize {
    debug_assert!(bytes[start] == b'$' && bytes.get(start + 1) == Some(&b'('));
    let len = bytes.len();
    let mut i = start + 2;
    let mut depth = 1;
    while i < len && depth > 0 {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'\\' if i + 1 < len => {
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    i
}

/// Advance past a single-quoted string starting at `bytes[i]` == `'`.
fn skip_single_quote(bytes: &[u8], start: usize) -> usize {
    let len = bytes.len();
    let mut i = start + 1;
    while i < len && bytes[i] != b'\'' {
        i += 1;
    }
    if i < len { i + 1 } else { i }
}

/// Advance past a double-quoted string starting at `bytes[i]` == `"`.
fn skip_double_quote(bytes: &[u8], start: usize) -> usize {
    let len = bytes.len();
    let mut i = start + 1;
    while i < len && bytes[i] != b'"' {
        if bytes[i] == b'\\' && i + 1 < len {
            i += 1;
        }
        i += 1;
    }
    if i < len { i + 1 } else { i }
}

/// Advance past a backtick command substitution starting at `bytes[i]` == `` ` ``.
fn skip_backtick(bytes: &[u8], start: usize) -> usize {
    let len = bytes.len();
    let mut i = start + 1;
    while i < len && bytes[i] != b'`' {
        if bytes[i] == b'\\' && i + 1 < len {
            i += 1;
        }
        i += 1;
    }
    if i < len { i + 1 } else { i }
}

/// Find the matching `{`...`}` pair at the top level, skipping `${` sequences
/// (parameter expansion), `$(` substitutions, quotes, and escapes.
fn find_brace_pair(input: &str) -> Option<(usize, usize)> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] {
            b'\\' => i = (i + 2).min(len),
            b'\'' => i = skip_single_quote(bytes, i),
            b'"' => i = skip_double_quote(bytes, i),
            b'`' => i = skip_backtick(bytes, i),
            b'$' if i + 1 < len && bytes[i + 1] == b'{' => i = skip_dollar_brace(bytes, i),
            b'$' if i + 1 < len && bytes[i + 1] == b'(' => i = skip_dollar_paren(bytes, i),
            b'{' => {
                if let Some(close) = find_matching_close(bytes, i) {
                    return Some((i, close));
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Given that `bytes[open]` is `{`, find the matching `}` respecting nesting,
/// quoting, and `${`/`$(` escapes.
fn find_matching_close(bytes: &[u8], open: usize) -> Option<usize> {
    let len = bytes.len();
    let mut depth: usize = 1;
    let mut i = open + 1;

    while i < len && depth > 0 {
        match bytes[i] {
            b'\\' => i = (i + 2).min(len),
            b'\'' => i = skip_single_quote(bytes, i),
            b'"' => i = skip_double_quote(bytes, i),
            b'`' => i = skip_backtick(bytes, i),
            b'$' if i + 1 < len && bytes[i + 1] == b'{' => i = skip_dollar_brace(bytes, i),
            b'$' if i + 1 < len && bytes[i + 1] == b'(' => i = skip_dollar_paren(bytes, i),
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Split the body of a brace expression by top-level commas.
///
/// Respects nested braces, quotes, escapes, and `${`/`$(`/backtick substitutions.
/// Content is extracted by slicing the original `&str` to preserve UTF-8.
fn split_alternatives(body: &str) -> Vec<String> {
    let bytes = body.as_bytes();
    let len = bytes.len();
    let mut parts = Vec::new();
    let mut seg_start = 0;
    let mut i = 0;
    let mut depth: usize = 0;

    while i < len {
        match bytes[i] {
            b'\\' => i = (i + 2).min(len),
            b'\'' => i = skip_single_quote(bytes, i),
            b'"' => i = skip_double_quote(bytes, i),
            b'`' => i = skip_backtick(bytes, i),
            b'$' if i + 1 < len && bytes[i + 1] == b'{' => i = skip_dollar_brace(bytes, i),
            b'$' if i + 1 < len && bytes[i + 1] == b'(' => i = skip_dollar_paren(bytes, i),
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b',' if depth == 0 => {
                parts.push(body[seg_start..i].to_string());
                i += 1;
                seg_start = i;
            }
            _ => i += 1,
        }
    }
    parts.push(body[seg_start..len].to_string());
    parts
}

/// Try to parse and expand a sequence expression: `a..b` or `a..b..step`.
fn try_sequence_expansion(body: &str) -> Option<Vec<String>> {
    let parts: Vec<&str> = body.split("..").collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }

    // Try numeric sequence
    if let (Ok(start), Ok(end)) = (parts[0].parse::<i64>(), parts[1].parse::<i64>()) {
        let step = if parts.len() == 3 {
            parts[2].parse::<i64>().ok()?.unsigned_abs() as i64
        } else {
            1
        };
        if step == 0 {
            return None;
        }

        let width = zero_pad_width(parts[0]).max(zero_pad_width(parts[1]));

        let mut result = Vec::new();
        if start <= end {
            let mut val = start;
            while val <= end {
                result.push(format_padded(val, width));
                val = match val.checked_add(step) {
                    Some(v) => v,
                    None => break,
                };
            }
        } else {
            let mut val = start;
            while val >= end {
                result.push(format_padded(val, width));
                val = match val.checked_sub(step) {
                    Some(v) => v,
                    None => break,
                };
            }
        }
        return Some(result);
    }

    // Try character sequence
    let start_ch = parse_single_char(parts[0])?;
    let end_ch = parse_single_char(parts[1])?;

    if !start_ch.is_ascii_alphanumeric() || !end_ch.is_ascii_alphanumeric() {
        return None;
    }

    let step = if parts.len() == 3 {
        parts[2].parse::<i64>().ok()?.unsigned_abs() as usize
    } else {
        1
    };
    if step == 0 {
        return None;
    }

    let start_u = start_ch as u32;
    let end_u = end_ch as u32;

    let mut result = Vec::new();
    if start_u <= end_u {
        let mut val = start_u;
        while val <= end_u {
            if let Some(c) = char::from_u32(val) {
                result.push(c.to_string());
            }
            val = match val.checked_add(step as u32) {
                Some(v) => v,
                None => break,
            };
        }
    } else {
        let mut val = start_u as i64;
        let end_i = end_u as i64;
        while val >= end_i {
            if let Some(c) = char::from_u32(val as u32) {
                result.push(c.to_string());
            }
            val = match val.checked_sub(step as i64) {
                Some(v) => v,
                None => break,
            };
        }
    }
    Some(result)
}

fn parse_single_char(s: &str) -> Option<char> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(c)
}

/// Determine zero-padding width from a numeric string.
fn zero_pad_width(s: &str) -> usize {
    let s = s.strip_prefix('-').unwrap_or(s);
    if s.len() > 1 && s.starts_with('0') {
        s.len()
    } else {
        0
    }
}

/// Format an integer with optional zero-padding.
fn format_padded(val: i64, width: usize) -> String {
    if width > 0 {
        if val < 0 {
            format!("-{:0>width$}", -val, width = width)
        } else {
            format!("{:0>width$}", val, width = width)
        }
    } else {
        val.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: usize = 10_000;

    #[test]
    fn comma_alternation() {
        assert_eq!(brace_expand("{a,b,c}", LIMIT).unwrap(), vec!["a", "b", "c"]);
    }

    #[test]
    fn comma_with_prefix_suffix() {
        assert_eq!(
            brace_expand("file{1,2,3}.txt", LIMIT).unwrap(),
            vec!["file1.txt", "file2.txt", "file3.txt"]
        );
    }

    #[test]
    fn prefix_and_suffix() {
        assert_eq!(
            brace_expand("pre{a,b}post", LIMIT).unwrap(),
            vec!["preapost", "prebpost"]
        );
    }

    #[test]
    fn nested_braces() {
        assert_eq!(
            brace_expand("{a,b{1,2}}", LIMIT).unwrap(),
            vec!["a", "b1", "b2"]
        );
    }

    #[test]
    fn deeply_nested() {
        assert_eq!(
            brace_expand("{a,{b,{c,d}}}", LIMIT).unwrap(),
            vec!["a", "b", "c", "d"]
        );
    }

    #[test]
    fn single_item_no_expansion() {
        assert_eq!(brace_expand("{a}", LIMIT).unwrap(), vec!["{a}"]);
    }

    #[test]
    fn empty_braces_no_expansion() {
        assert_eq!(brace_expand("{}", LIMIT).unwrap(), vec!["{}"]);
    }

    #[test]
    fn empty_alternative() {
        assert_eq!(brace_expand("{a,}", LIMIT).unwrap(), vec!["a", ""]);
    }

    #[test]
    fn two_empty_alternatives() {
        assert_eq!(brace_expand("{,}", LIMIT).unwrap(), vec!["", ""]);
    }

    #[test]
    fn backup_idiom() {
        assert_eq!(
            brace_expand("file{,.bak}", LIMIT).unwrap(),
            vec!["file", "file.bak"]
        );
    }

    #[test]
    fn numeric_sequence() {
        assert_eq!(
            brace_expand("{1..5}", LIMIT).unwrap(),
            vec!["1", "2", "3", "4", "5"]
        );
    }

    #[test]
    fn numeric_sequence_reverse() {
        assert_eq!(
            brace_expand("{5..1}", LIMIT).unwrap(),
            vec!["5", "4", "3", "2", "1"]
        );
    }

    #[test]
    fn numeric_sequence_with_step() {
        assert_eq!(
            brace_expand("{1..10..2}", LIMIT).unwrap(),
            vec!["1", "3", "5", "7", "9"]
        );
    }

    #[test]
    fn numeric_sequence_negative_step_ignored() {
        assert_eq!(
            brace_expand("{5..1..-2}", LIMIT).unwrap(),
            vec!["5", "3", "1"]
        );
    }

    #[test]
    fn sequence_single_element() {
        assert_eq!(brace_expand("{3..3}", LIMIT).unwrap(), vec!["3"]);
    }

    #[test]
    fn char_sequence() {
        let result = brace_expand("{a..z}", LIMIT).unwrap();
        assert_eq!(result.len(), 26);
        assert_eq!(result[0], "a");
        assert_eq!(result[25], "z");
    }

    #[test]
    fn char_sequence_reverse() {
        let result = brace_expand("{z..a}", LIMIT).unwrap();
        assert_eq!(result.len(), 26);
        assert_eq!(result[0], "z");
        assert_eq!(result[25], "a");
    }

    #[test]
    fn char_sequence_with_step() {
        assert_eq!(
            brace_expand("{a..z..5}", LIMIT).unwrap(),
            vec!["a", "f", "k", "p", "u", "z"]
        );
    }

    #[test]
    fn parameter_expansion_not_affected() {
        assert_eq!(brace_expand("${VAR}", LIMIT).unwrap(), vec!["${VAR}"]);
    }

    #[test]
    fn parameter_expansion_with_braces() {
        assert_eq!(
            brace_expand("${X}{a,b}", LIMIT).unwrap(),
            vec!["${X}a", "${X}b"]
        );
    }

    #[test]
    fn command_substitution_not_affected() {
        assert_eq!(
            brace_expand("$(echo hi)", LIMIT).unwrap(),
            vec!["$(echo hi)"]
        );
    }

    #[test]
    fn command_substitution_with_comma_in_alternatives() {
        assert_eq!(
            brace_expand("{$(echo a,b),c}", LIMIT).unwrap(),
            vec!["$(echo a,b)", "c"]
        );
    }

    #[test]
    fn unmatched_brace_literal() {
        assert_eq!(brace_expand("{a", LIMIT).unwrap(), vec!["{a"]);
    }

    #[test]
    fn limit_exceeded() {
        let result = brace_expand("{1..20000}", 100);
        assert!(result.is_err());
    }

    #[test]
    fn numeric_zero_padded() {
        assert_eq!(
            brace_expand("{01..03}", LIMIT).unwrap(),
            vec!["01", "02", "03"]
        );
    }

    #[test]
    fn negative_range() {
        assert_eq!(
            brace_expand("{-2..2}", LIMIT).unwrap(),
            vec!["-2", "-1", "0", "1", "2"]
        );
    }

    #[test]
    fn multiple_brace_groups() {
        assert_eq!(
            brace_expand("{a,b}{1,2}", LIMIT).unwrap(),
            vec!["a1", "a2", "b1", "b2"]
        );
    }

    #[test]
    fn non_ascii_content() {
        assert_eq!(
            brace_expand("{café,bar}", LIMIT).unwrap(),
            vec!["café", "bar"]
        );
    }
}
