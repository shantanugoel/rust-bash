//! Shared regex utilities for grep, sed, and other commands.
//!
//! Provides BRE-to-ERE translation since the `regex` crate uses ERE semantics natively.

/// Convert a POSIX Basic Regular Expression (BRE) to an Extended Regular Expression (ERE).
///
/// BRE differences from ERE:
/// - `\(` and `\)` are grouping (ERE uses bare `(` `)`)
/// - `\{` and `\}` are interval (ERE uses bare `{` `}`)
/// - `+`, `?`, `|` are literal in BRE (must be escaped as `\+`, `\?`, `\|` for special meaning)
///
/// This function performs best-effort translation:
/// - `\(` → `(`, `\)` → `)`, `\{` → `{`, `\}` → `}`
/// - Bare `(`, `)`, `{`, `}` → escaped `\(`, `\)`, `\{`, `\}`
/// - Bare `+`, `?`, `|` → escaped `\+`, `\?`, `\|`
/// - `\+` → `+`, `\?` → `?`, `\|` → `|`
pub fn bre_to_ere(bre: &str) -> String {
    let bytes = bre.as_bytes();
    let mut ere = String::with_capacity(bre.len());
    let mut i = 0;
    let mut in_bracket = false;

    while i < bytes.len() {
        // Inside character classes, pass through verbatim
        if in_bracket {
            ere.push(bytes[i] as char);
            if bytes[i] == b']' && i > 0 {
                in_bracket = false;
            }
            i += 1;
            continue;
        }

        // Detect start of character class
        if bytes[i] == b'[' {
            ere.push('[');
            in_bracket = true;
            i += 1;
            // Handle `[^` and `[]` or `[^]` (] as first char in class is literal)
            if i < bytes.len() && bytes[i] == b'^' {
                ere.push('^');
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b']' {
                ere.push(']');
                i += 1;
            }
            continue;
        }

        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            match bytes[i + 1] {
                b'(' => ere.push('('),
                b')' => ere.push(')'),
                b'{' => ere.push('{'),
                b'}' => ere.push('}'),
                b'+' => ere.push('+'),
                b'?' => ere.push('?'),
                b'|' => ere.push('|'),
                other => {
                    ere.push('\\');
                    ere.push(other as char);
                }
            }
            i += 2;
        } else {
            match bytes[i] {
                b'(' => ere.push_str("\\("),
                b')' => ere.push_str("\\)"),
                b'{' => ere.push_str("\\{"),
                b'}' => ere.push_str("\\}"),
                b'+' => ere.push_str("\\+"),
                b'?' => ere.push_str("\\?"),
                b'|' => ere.push_str("\\|"),
                other => ere.push(other as char),
            }
            i += 1;
        }
    }

    ere
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bre_groups_to_ere() {
        assert_eq!(bre_to_ere(r"\(abc\)"), "(abc)");
    }

    #[test]
    fn bre_intervals_to_ere() {
        assert_eq!(bre_to_ere(r"a\{2,3\}"), "a{2,3}");
    }

    #[test]
    fn bre_bare_parens_escaped() {
        assert_eq!(bre_to_ere("(abc)"), r"\(abc\)");
    }

    #[test]
    fn bre_plus_question_literal_by_default() {
        assert_eq!(bre_to_ere("a+b?c"), r"a\+b\?c");
    }

    #[test]
    fn bre_escaped_plus_becomes_quantifier() {
        assert_eq!(bre_to_ere(r"a\+"), "a+");
    }

    #[test]
    fn bre_alternation() {
        assert_eq!(bre_to_ere(r"a\|b"), "a|b");
    }

    #[test]
    fn bre_bare_pipe_escaped() {
        assert_eq!(bre_to_ere("a|b"), r"a\|b");
    }

    #[test]
    fn bre_mixed_escapes() {
        assert_eq!(bre_to_ere(r"\(a\+\|b\)"), "(a+|b)");
    }

    #[test]
    fn bre_regular_escapes_preserved() {
        assert_eq!(bre_to_ere(r"\d\w\."), r"\d\w\.");
    }

    #[test]
    fn bre_empty_string() {
        assert_eq!(bre_to_ere(""), "");
    }

    #[test]
    fn bre_trailing_backslash() {
        // A trailing backslash is passed through literally
        assert_eq!(bre_to_ere("abc\\"), "abc\\");
    }

    #[test]
    fn bre_character_class_unchanged() {
        // Characters inside [...] are not transformed
        assert_eq!(bre_to_ere("[+?|()]"), "[+?|()]");
    }

    #[test]
    fn bre_negated_character_class() {
        assert_eq!(bre_to_ere("[^+?]"), "[^+?]");
    }

    #[test]
    fn bre_bracket_with_closing_first() {
        // ] as first char in class is literal
        assert_eq!(bre_to_ere("[]ab]"), "[]ab]");
    }
}
