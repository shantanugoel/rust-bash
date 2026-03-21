//! Simple shell-glob pattern matching for parameter expansion operators.
//!
//! Supports `*`, `?`, `[...]` (character classes), `[!...]` (negated classes),
//! and literal characters. Backslash escapes the next character.

/// Match a shell glob pattern against a string.
pub(crate) fn glob_match(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes(), false)
}

/// Case-insensitive variant of `glob_match`.
pub(crate) fn glob_match_nocase(pattern: &str, text: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), text.as_bytes(), true)
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

fn glob_match_inner(pat: &[u8], txt: &[u8], nocase: bool) -> bool {
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
            // `?` matches one character, advance by its full UTF-8 byte length
            pi += 1;
            ti += utf8_char_len(txt[ti]);
            continue;
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
        if i + 2 < pat.len() && pat[i + 1] == b'-' && pat[i + 2] != b']' {
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

/// Find the shortest suffix of `text` matching `pattern`.
/// Returns the index where the matched suffix starts, or None.
pub(crate) fn shortest_suffix_match(text: &str, pattern: &str) -> Option<usize> {
    for i in (0..=text.len()).rev() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if glob_match(pattern, &text[i..]) {
            return Some(i);
        }
    }
    None
}

/// Find the longest suffix of `text` matching `pattern`.
/// Returns the index where the matched suffix starts, or None.
pub(crate) fn longest_suffix_match(text: &str, pattern: &str) -> Option<usize> {
    for i in 0..=text.len() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if glob_match(pattern, &text[i..]) {
            return Some(i);
        }
    }
    None
}

/// Find the shortest prefix of `text` matching `pattern`.
/// Returns the length of the matched prefix, or None.
pub(crate) fn shortest_prefix_match(text: &str, pattern: &str) -> Option<usize> {
    for i in 0..=text.len() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if glob_match(pattern, &text[..i]) {
            return Some(i);
        }
    }
    None
}

/// Find the longest prefix of `text` matching `pattern`.
/// Returns the length of the matched prefix, or None.
pub(crate) fn longest_prefix_match(text: &str, pattern: &str) -> Option<usize> {
    for i in (0..=text.len()).rev() {
        if !text.is_char_boundary(i) {
            continue;
        }
        if glob_match(pattern, &text[..i]) {
            return Some(i);
        }
    }
    None
}

/// Find the first occurrence of `pattern` in `text` (longest match at earliest position).
/// Returns `(start, end)` of the match, or None.
pub(crate) fn first_match(text: &str, pattern: &str) -> Option<(usize, usize)> {
    for start in 0..=text.len() {
        if !text.is_char_boundary(start) {
            continue;
        }
        // Try longest match first at this start position
        for end in (start..=text.len()).rev() {
            if !text.is_char_boundary(end) {
                continue;
            }
            if glob_match(pattern, &text[start..end]) {
                return Some((start, end));
            }
        }
    }
    None
}

/// Replace all occurrences of `pattern` in `text` with `replacement`.
pub(crate) fn replace_all(text: &str, pattern: &str, replacement: &str) -> String {
    let mut result = String::new();
    let mut i = 0;
    while i < text.len() {
        let mut found = false;
        for end in (i + 1..=text.len()).rev() {
            if glob_match(pattern, &text[i..end]) {
                result.push_str(replacement);
                i = end;
                found = true;
                break;
            }
        }
        if !found {
            // Advance by one character (not one byte)
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
}
