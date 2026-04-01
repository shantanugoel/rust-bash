//! Shell-glob pattern matching for parameter expansion and `[[ ]]`.
//!
//! Supports `*`, `?`, `[...]` (character classes), `[!...]` (negated classes),
//! literal characters, and extglob patterns: `@(...)`, `+(...)`, `*(...)`,
//! `?(...)`, `!(...)`. Backslash escapes the next character.
//!
//! Character classes also support POSIX named classes like `[:alpha:]`.

/// Match a shell glob pattern against a string (no extglob).
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes(), false, false)
}

/// Case-insensitive variant of `glob_match` (no extglob).
pub(crate) fn glob_match_nocase(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes(), true, false)
}

/// Path-aware glob match: `*` does not match `/` (for GLOBIGNORE, file globbing).
pub(crate) fn glob_match_path(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes(), false, true)
}

/// Match a shell glob pattern with extglob support.
pub(crate) fn extglob_match(pattern: &str, text: &str) -> bool {
    if has_extglob_syntax(pattern.as_bytes()) {
        return ext_match(pattern.as_bytes(), 0, text.as_bytes(), 0, false, 0);
    }
    glob_match_inner(pattern.as_bytes(), text.as_bytes(), false, false)
}

/// Case-insensitive extglob match.
pub(crate) fn extglob_match_nocase(pattern: &str, text: &str) -> bool {
    if has_extglob_syntax(pattern.as_bytes()) {
        return ext_match(pattern.as_bytes(), 0, text.as_bytes(), 0, true, 0);
    }
    glob_match_inner(pattern.as_bytes(), text.as_bytes(), true, false)
}

/// Return the byte length of the UTF-8 character starting at `first_byte`.
fn utf8_char_len(first_byte: u8) -> usize {
    if first_byte < 0x80 {
        1
    } else if first_byte < 0xE0 {
        2
    } else if first_byte < 0xF0 {
        3
    } else {
        4
    }
}

fn glob_match_inner(pat: &[u8], txt: &[u8], nocase: bool, path_mode: bool) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;

    while ti < txt.len() {
        if pi < pat.len() && pat[pi] == b'\\' && pi + 1 < pat.len() {
            // escaped literal
            if bytes_eq(txt[ti], pat[pi + 1], nocase) {
                pi += 2;
                ti += 1;
                continue;
            }
        } else if pi < pat.len() && pat[pi] == b'?' {
            // In path mode, `?` does not match `/`
            if path_mode && txt[ti] == b'/' {
                // fall through to mismatch/backtrack
            } else {
                // `?` matches one character, advance by its full UTF-8 byte length
                pi += 1;
                ti += utf8_char_len(txt[ti]);
                continue;
            }
        } else if pi < pat.len() && pat[pi] == b'[' {
            if let Some((matched, end)) = match_char_class(&pat[pi..], txt[ti], nocase)
                && matched
            {
                pi += end;
                ti += 1;
                continue;
            }
        } else if pi < pat.len() && pat[pi] == b'*' {
            star_pi = pi;
            star_ti = ti;
            pi += 1;
            continue;
        } else if pi < pat.len() && bytes_eq(pat[pi], txt[ti], nocase) {
            pi += 1;
            ti += 1;
            continue;
        }

        // Mismatch — backtrack to last star if possible
        if star_pi != usize::MAX {
            // In path mode, `*` cannot cross `/`
            if path_mode && txt[star_ti] == b'/' {
                return false;
            }
            pi = star_pi + 1;
            // Advance star_ti by one full UTF-8 character
            star_ti += utf8_char_len(txt[star_ti]);
            ti = star_ti;
            continue;
        }

        return false;
    }

    // Consume trailing stars
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }

    pi == pat.len()
}

/// Compare two bytes, optionally case-insensitive (ASCII only).
fn bytes_eq(a: u8, b: u8, nocase: bool) -> bool {
    if nocase {
        a.eq_ignore_ascii_case(&b)
    } else {
        a == b
    }
}

/// Attempt to match a character class `[...]` at the start of `pat`.
/// Returns `(matched, bytes_consumed)` or `None` if not a valid class.
/// Supports POSIX named classes like `[:alpha:]`, `[:digit:]`, etc.
fn match_char_class(pat: &[u8], ch: u8, nocase: bool) -> Option<(bool, usize)> {
    if pat.is_empty() || pat[0] != b'[' {
        return None;
    }
    let mut i = 1;
    let negated = if i < pat.len() && (pat[i] == b'!' || pat[i] == b'^') {
        i += 1;
        true
    } else {
        false
    };

    let mut matched = false;
    // Allow ] as first char in class
    if i < pat.len() && pat[i] == b']' {
        if bytes_eq(ch, b']', nocase) {
            matched = true;
        }
        i += 1;
    }

    while i < pat.len() && pat[i] != b']' {
        // POSIX named class [:name:]
        if pat[i] == b'['
            && i + 1 < pat.len()
            && pat[i + 1] == b':'
            && let Some(end) = find_posix_class_end(pat, i)
        {
            let class_name = &pat[i + 2..end - 1]; // between [: and :]
            if posix_class_matches(class_name, ch) {
                matched = true;
            }
            i = end + 1; // skip past :]
            continue;
        }
        if pat[i] == b'\\' && i + 1 < pat.len() {
            // Escaped character inside class (including \])
            if bytes_eq(pat[i + 1], ch, nocase) {
                matched = true;
            }
            i += 2;
        } else if i + 2 < pat.len() && pat[i + 1] == b'-' && pat[i + 2] != b']' {
            let lo = pat[i];
            let hi = pat[i + 2];
            if nocase {
                let ch_lower = ch.to_ascii_lowercase();
                if ch_lower >= lo.to_ascii_lowercase() && ch_lower <= hi.to_ascii_lowercase() {
                    matched = true;
                }
            } else if ch >= lo && ch <= hi {
                matched = true;
            }
            i += 3;
        } else {
            if bytes_eq(pat[i], ch, nocase) {
                matched = true;
            }
            i += 1;
        }
    }

    if i >= pat.len() {
        return None; // unclosed bracket
    }

    // i is at ']'
    let result = if negated { !matched } else { matched };
    Some((result, i + 1))
}

/// Find the end of a POSIX character class `[:name:]` starting at `start`.
/// Returns the index of the closing `]` of `:]`.
fn find_posix_class_end(pat: &[u8], start: usize) -> Option<usize> {
    // start points to '[', start+1 is ':'
    let mut i = start + 2;
    while i < pat.len() {
        if pat[i] == b':' && i + 1 < pat.len() && pat[i + 1] == b']' {
            return Some(i + 1);
        }
        if !pat[i].is_ascii_alphanumeric() {
            return None;
        }
        i += 1;
    }
    None
}

/// Check if a byte matches a POSIX named character class.
fn posix_class_matches(name: &[u8], ch: u8) -> bool {
    match name {
        b"alpha" => ch.is_ascii_alphabetic(),
        b"digit" => ch.is_ascii_digit(),
        b"alnum" => ch.is_ascii_alphanumeric(),
        b"upper" => ch.is_ascii_uppercase(),
        b"lower" => ch.is_ascii_lowercase(),
        b"space" => ch.is_ascii_whitespace(),
        b"blank" => ch == b' ' || ch == b'\t',
        b"print" => (0x20..=0x7e).contains(&ch),
        b"graph" => ch > 0x20 && ch <= 0x7e,
        b"cntrl" => ch < 0x20 || ch == 0x7f,
        b"punct" => ch.is_ascii_punctuation(),
        b"xdigit" => ch.is_ascii_hexdigit(),
        b"ascii" => ch.is_ascii(),
        _ => false,
    }
}

// ── Extglob matching ──────────────────────────────────────────────

const MAX_EXTGLOB_DEPTH: usize = 64;

/// Bundles the pattern, text, and matching options for extglob routines.
struct ExtMatchCtx<'a> {
    pat: &'a [u8],
    txt: &'a [u8],
    nocase: bool,
}

/// Check if pattern bytes contain extglob syntax.
fn has_extglob_syntax(pat: &[u8]) -> bool {
    let mut i = 0;
    while i < pat.len() {
        if pat[i] == b'\\' {
            i += 2;
            continue;
        }
        if i + 1 < pat.len()
            && matches!(pat[i], b'@' | b'+' | b'*' | b'?' | b'!')
            && pat[i + 1] == b'('
            && find_matching_paren(pat, i + 2).is_some()
        {
            return true;
        }
        i += 1;
    }
    false
}

/// Find the matching closing paren for an extglob group.
/// `start` is the index right after the opening `(`.
fn find_matching_paren(pat: &[u8], start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = start;
    while i < pat.len() {
        if pat[i] == b'\\' && i + 1 < pat.len() {
            i += 2;
            continue;
        }
        if i + 1 < pat.len()
            && matches!(pat[i], b'@' | b'+' | b'*' | b'?' | b'!')
            && pat[i + 1] == b'('
        {
            depth += 1;
            i += 2;
            continue;
        }
        if pat[i] == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

/// Split extglob alternatives at top-level pipes.
fn split_at_pipes(pat: &[u8]) -> Vec<&[u8]> {
    let mut result = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < pat.len() {
        if pat[i] == b'\\' && i + 1 < pat.len() {
            i += 2;
            continue;
        }
        if i + 1 < pat.len()
            && matches!(pat[i], b'@' | b'+' | b'*' | b'?' | b'!')
            && pat[i + 1] == b'('
            && let Some(close) = find_matching_paren(pat, i + 2)
        {
            i = close + 1;
            continue;
        }
        if pat[i] == b'|' {
            result.push(&pat[start..i]);
            start = i + 1;
        }
        i += 1;
    }
    result.push(&pat[start..]);
    result
}

/// Try to parse an extglob operator at position `pi`.
/// Returns `(op, inner_start, inner_end, after_close_index)`.
fn try_extglob_at(pat: &[u8], pi: usize) -> Option<(u8, usize, usize, usize)> {
    if pi + 1 >= pat.len() {
        return None;
    }
    let op = pat[pi];
    if !matches!(op, b'@' | b'+' | b'*' | b'?' | b'!') {
        return None;
    }
    if pat[pi + 1] != b'(' {
        return None;
    }
    let inner_start = pi + 2;
    find_matching_paren(pat, inner_start).map(|close| (op, inner_start, close, close + 1))
}

/// Core recursive matching with extglob support.
/// Returns true if `pat[pi..]` matches `txt[ti..]` fully.
fn ext_match(pat: &[u8], pi: usize, txt: &[u8], ti: usize, nocase: bool, depth: usize) -> bool {
    if depth > MAX_EXTGLOB_DEPTH {
        return false;
    }

    if pi >= pat.len() {
        return ti >= txt.len();
    }

    // Escape sequence
    if pat[pi] == b'\\' && pi + 1 < pat.len() {
        if ti < txt.len() && bytes_eq(pat[pi + 1], txt[ti], nocase) {
            return ext_match(pat, pi + 2, txt, ti + 1, nocase, depth);
        }
        return false;
    }

    // Try extglob
    if let Some((op, inner_start, inner_end, after)) = try_extglob_at(pat, pi) {
        let alts = split_at_pipes(&pat[inner_start..inner_end]);
        return ext_match_group(
            &ExtMatchCtx { pat, txt, nocase },
            after,
            ti,
            op,
            &alts,
            depth,
        );
    }

    // ? wildcard (only if not extglob ?(...)  — already handled above)
    if pat[pi] == b'?' {
        if ti < txt.len() {
            return ext_match(pat, pi + 1, txt, ti + utf8_char_len(txt[ti]), nocase, depth);
        }
        return false;
    }

    // [...] character class
    if pat[pi] == b'[' {
        if ti < txt.len()
            && let Some((matched, end)) = match_char_class(&pat[pi..], txt[ti], nocase)
            && matched
        {
            return ext_match(pat, pi + end, txt, ti + 1, nocase, depth);
        }
        return false;
    }

    // * wildcard (only if not extglob *(...)  — already handled above)
    if pat[pi] == b'*' {
        let mut np = pi + 1;
        while np < pat.len() && pat[np] == b'*' {
            // Don't skip if this * starts an extglob *(
            if np + 1 < pat.len() && pat[np + 1] == b'(' {
                break;
            }
            np += 1;
        }
        for t in ti..=txt.len() {
            if ext_match(pat, np, txt, t, nocase, depth + 1) {
                return true;
            }
        }
        return false;
    }

    // Literal character
    if ti < txt.len() && bytes_eq(pat[pi], txt[ti], nocase) {
        return ext_match(pat, pi + 1, txt, ti + 1, nocase, depth);
    }

    false
}

/// Handle extglob operator matching at position `ti` in text.
fn ext_match_group(
    ctx: &ExtMatchCtx<'_>,
    after: usize,
    ti: usize,
    op: u8,
    alts: &[&[u8]],
    depth: usize,
) -> bool {
    if depth > MAX_EXTGLOB_DEPTH {
        return false;
    }
    let remaining = ctx.txt.len() - ti;

    match op {
        b'@' => {
            // Exactly one alternative must match a prefix, then rest matches
            for alt in alts {
                for len in 0..=remaining {
                    if ext_match(alt, 0, &ctx.txt[ti..ti + len], 0, ctx.nocase, depth + 1)
                        && ext_match(ctx.pat, after, ctx.txt, ti + len, ctx.nocase, depth + 1)
                    {
                        return true;
                    }
                }
            }
            false
        }
        b'?' => {
            // Zero or one alternative
            if ext_match(ctx.pat, after, ctx.txt, ti, ctx.nocase, depth + 1) {
                return true;
            }
            for alt in alts {
                for len in 0..=remaining {
                    if ext_match(alt, 0, &ctx.txt[ti..ti + len], 0, ctx.nocase, depth + 1)
                        && ext_match(ctx.pat, after, ctx.txt, ti + len, ctx.nocase, depth + 1)
                    {
                        return true;
                    }
                }
            }
            false
        }
        b'+' => ext_match_repeat(ctx, after, ti, alts, depth, 1),
        b'*' => ext_match_repeat(ctx, after, ti, alts, depth, 0),
        b'!' => {
            // Match anything that does NOT match any alternative
            for len in 0..=remaining {
                let slice = &ctx.txt[ti..ti + len];
                let any_match = alts
                    .iter()
                    .any(|alt| ext_match(alt, 0, slice, 0, ctx.nocase, depth + 1));
                if !any_match && ext_match(ctx.pat, after, ctx.txt, ti + len, ctx.nocase, depth + 1)
                {
                    return true;
                }
            }
            false
        }
        _ => false,
    }
}

/// Match one or more (`min_count >= 1`) or zero or more (`min_count == 0`)
/// alternatives, then match the rest of the pattern.
fn ext_match_repeat(
    ctx: &ExtMatchCtx<'_>,
    after: usize,
    ti: usize,
    alts: &[&[u8]],
    depth: usize,
    min_count: usize,
) -> bool {
    if depth > MAX_EXTGLOB_DEPTH {
        return false;
    }
    // Try matching rest if we've satisfied the minimum
    if min_count == 0 && ext_match(ctx.pat, after, ctx.txt, ti, ctx.nocase, depth + 1) {
        return true;
    }
    // Try matching one alternative, then repeat
    let remaining = ctx.txt.len() - ti;
    for alt in alts {
        for len in 1..=remaining {
            if ext_match(alt, 0, &ctx.txt[ti..ti + len], 0, ctx.nocase, depth + 1) {
                let new_min = min_count.saturating_sub(1);
                if ext_match_repeat(ctx, after, ti + len, alts, depth + 1, new_min) {
                    return true;
                }
            }
        }
    }
    false
}

/// Choose the appropriate match function based on extglob flag.
fn do_match(pattern: &str, text: &str, extglob: bool) -> bool {
    if extglob {
        extglob_match(pattern, text)
    } else {
        glob_match(pattern, text)
    }
}

/// Find the shortest suffix of `text` matching `pattern`.
/// Returns the index where the matched suffix starts, or None.
#[cfg(test)]
pub(crate) fn shortest_suffix_match(text: &str, pattern: &str) -> Option<usize> {
    shortest_suffix_match_ext(text, pattern, false)
}

pub(crate) fn shortest_suffix_match_ext(text: &str, pattern: &str, extglob: bool) -> Option<usize> {
    for i in (0..=text.len()).rev() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if do_match(pattern, &text[i..], extglob) {
            return Some(i);
        }
    }
    None
}

/// Find the longest suffix of `text` matching `pattern`.
/// Returns the index where the matched suffix starts, or None.
#[cfg(test)]
pub(crate) fn longest_suffix_match(text: &str, pattern: &str) -> Option<usize> {
    longest_suffix_match_ext(text, pattern, false)
}

pub(crate) fn longest_suffix_match_ext(text: &str, pattern: &str, extglob: bool) -> Option<usize> {
    for i in 0..=text.len() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if do_match(pattern, &text[i..], extglob) {
            return Some(i);
        }
    }
    None
}

/// Find the shortest prefix of `text` matching `pattern`.
/// Returns the length of the matched prefix, or None.
#[cfg(test)]
pub(crate) fn shortest_prefix_match(text: &str, pattern: &str) -> Option<usize> {
    shortest_prefix_match_ext(text, pattern, false)
}

pub(crate) fn shortest_prefix_match_ext(text: &str, pattern: &str, extglob: bool) -> Option<usize> {
    for i in 0..=text.len() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if do_match(pattern, &text[..i], extglob) {
            return Some(i);
        }
    }
    None
}

/// Find the longest prefix of `text` matching `pattern`.
/// Returns the length of the matched prefix, or None.
#[cfg(test)]
pub(crate) fn longest_prefix_match(text: &str, pattern: &str) -> Option<usize> {
    longest_prefix_match_ext(text, pattern, false)
}

pub(crate) fn longest_prefix_match_ext(text: &str, pattern: &str, extglob: bool) -> Option<usize> {
    for i in (0..=text.len()).rev() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if do_match(pattern, &text[..i], extglob) {
            return Some(i);
        }
    }
    None
}

/// Find the first occurrence of `pattern` in `text` (longest match at earliest position).
/// Returns `(start, end)` of the match, or None.
#[cfg(test)]
pub(crate) fn first_match(text: &str, pattern: &str) -> Option<(usize, usize)> {
    first_match_ext(text, pattern, false)
}

pub(crate) fn first_match_ext(text: &str, pattern: &str, extglob: bool) -> Option<(usize, usize)> {
    for start in 0..=text.len() {
        if !text.is_char_boundary(start) {
            continue;
        }
        for end in (start..=text.len()).rev() {
            if !text.is_char_boundary(end) {
                continue;
            }
            if do_match(pattern, &text[start..end], extglob) {
                return Some((start, end));
            }
        }
    }
    None
}

/// Replace all occurrences of `pattern` in `text` with `replacement`.
#[cfg(test)]
pub(crate) fn replace_all(text: &str, pattern: &str, replacement: &str) -> String {
    replace_all_ext(text, pattern, replacement, false)
}

pub(crate) fn replace_all_ext(
    text: &str,
    pattern: &str,
    replacement: &str,
    extglob: bool,
) -> String {
    let mut result = String::new();
    let mut i = 0;
    while i < text.len() {
        let mut found = false;
        for end in (i + 1..=text.len()).rev() {
            if do_match(pattern, &text[i..end], extglob) {
                result.push_str(replacement);
                i = end;
                found = true;
                break;
            }
        }
        if !found {
            if let Some(ch) = text[i..].chars().next() {
                result.push(ch);
                i += ch.len_utf8();
            } else {
                i += 1;
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_match() {
        assert!(glob_match("hello", "hello"));
        assert!(!glob_match("hello", "world"));
    }

    #[test]
    fn star_match() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("h*o", "hello"));
        assert!(glob_match("h*o", "ho"));
        assert!(!glob_match("h*o", "help"));
    }

    #[test]
    fn question_match() {
        assert!(glob_match("h?llo", "hello"));
        assert!(!glob_match("h?llo", "hllo"));
    }

    #[test]
    fn char_class() {
        assert!(glob_match("[abc]", "a"));
        assert!(glob_match("[a-z]", "m"));
        assert!(!glob_match("[a-z]", "M"));
        assert!(glob_match("[!a-z]", "M"));
    }

    #[test]
    fn suffix_removal() {
        assert_eq!(shortest_suffix_match("hello.tar.gz", ".*"), Some(9));
        assert_eq!(longest_suffix_match("hello.tar.gz", ".*"), Some(5));
    }

    #[test]
    fn prefix_removal() {
        // bash: ${x#*/} on "/a/b/c" removes "/" → "a/b/c" (1 char prefix)
        assert_eq!(shortest_prefix_match("/a/b/c", "*/"), Some(1));
        // bash: ${x##*/} on "/a/b/c" removes "/a/b/" → "c" (5 char prefix)
        assert_eq!(longest_prefix_match("/a/b/c", "*/"), Some(5));
    }

    #[test]
    fn first_match_basic() {
        assert_eq!(first_match("hello world", "o"), Some((4, 5)));
    }

    #[test]
    fn replace_all_basic() {
        assert_eq!(replace_all("hello", "l", "r"), "herro");
    }

    // ── Extglob tests ──

    #[test]
    fn extglob_at() {
        assert!(extglob_match("--@(help|verbose)", "--verbose"));
        assert!(extglob_match("--@(help|verbose)", "--help"));
        assert!(!extglob_match("--@(help|verbose)", "--oops"));
        assert!(extglob_match("@(cc)", "cc"));
    }

    #[test]
    fn extglob_question() {
        assert!(extglob_match("?(a|b)", ""));
        assert!(extglob_match("?(a|b)", "a"));
        assert!(!extglob_match("?(a|b)", "ab"));
    }

    #[test]
    fn extglob_plus() {
        assert!(extglob_match("+(foo)", "foo"));
        assert!(extglob_match("+(foo)", "foofoo"));
        assert!(!extglob_match("+(foo)", ""));
    }

    #[test]
    fn extglob_star() {
        assert!(extglob_match("*(foo)", ""));
        assert!(extglob_match("*(foo)", "foo"));
        assert!(extglob_match("*(foo)", "foofoo"));
    }

    #[test]
    fn extglob_not() {
        assert!(extglob_match("!(dog)", "cat"));
        assert!(!extglob_match("!(dog)", "dog"));
        assert!(extglob_match("!(dog)", ""));
    }

    #[test]
    fn extglob_with_glob() {
        assert!(extglob_match("@(*.c|*.h)", "foo.c"));
        assert!(extglob_match("@(*.c|*.h)", "bar.h"));
        assert!(!extglob_match("@(*.c|*.h)", "baz.o"));
    }

    #[test]
    fn extglob_nested() {
        assert!(extglob_match("@(a|@(b|c))", "b"));
        assert!(extglob_match("@(a|@(b|c))", "c"));
    }

    // ── POSIX character class tests ──

    #[test]
    fn posix_char_class() {
        assert!(glob_match("[[:alpha:]]", "a"));
        assert!(!glob_match("[[:alpha:]]", "1"));
        assert!(glob_match("[[:digit:]]", "5"));
        assert!(!glob_match("[[:digit:]]", "x"));
    }
}
