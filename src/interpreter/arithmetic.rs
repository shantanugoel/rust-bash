//! Arithmetic expression evaluator for `$((...))`, `(( ))`, `let`, and
//! C-style `for (( ; ; ))` loops.
//!
//! Implements a recursive-descent parser that handles all bash arithmetic
//! operators with correct precedence.

use crate::error::RustBashError;
use crate::interpreter::{InterpreterState, set_variable};

// ── Public API ──────────────────────────────────────────────────────

/// Evaluate an arithmetic expression string, returning its i64 result.
/// Variables are read from / written to `state.env`.
pub(crate) fn eval_arithmetic(
    expr: &str,
    state: &mut InterpreterState,
) -> Result<i64, RustBashError> {
    let tokens = tokenize(expr)?;
    if tokens.is_empty() {
        return Ok(0);
    }
    let mut parser = Parser::with_source(&tokens, expr);
    let result = parser.parse_comma(state)?;
    if parser.pos < parser.tokens.len() {
        return Err(RustBashError::Execution(format!(
            "arithmetic: unexpected token near `{}`",
            parser.tokens[parser.pos].text(expr)
        )));
    }
    Ok(result)
}

// ── Tokens ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokenKind {
    Number(i64),
    Ident,      // variable name (start, len stored separately)
    Plus,       // +
    Minus,      // -
    Star,       // *
    StarStar,   // **
    Slash,      // /
    Percent,    // %
    Amp,        // &
    AmpAmp,     // &&
    Pipe,       // |
    PipePipe,   // ||
    Caret,      // ^
    Tilde,      // ~
    Bang,       // !
    Eq,         // =
    EqEq,       // ==
    BangEq,     // !=
    Lt,         // <
    LtEq,       // <=
    LtLt,       // <<
    Gt,         // >
    GtEq,       // >=
    GtGt,       // >>
    PlusEq,     // +=
    MinusEq,    // -=
    StarEq,     // *=
    SlashEq,    // /=
    PercentEq,  // %=
    LtLtEq,     // <<=
    GtGtEq,     // >>=
    AmpEq,      // &=
    PipeEq,     // |=
    CaretEq,    // ^=
    PlusPlus,   // ++
    MinusMinus, // --
    Question,   // ?
    Colon,      // :
    LParen,     // (
    RParen,     // )
    LBracket,   // [
    RBracket,   // ]
    Comma,      // ,
}

#[derive(Debug, Clone, Copy)]
struct Token {
    kind: TokenKind,
    start: usize,
    len: usize,
}

impl Token {
    fn text<'a>(&self, source: &'a str) -> &'a str {
        &source[self.start..self.start + self.len]
    }
}

// ── Tokenizer ───────────────────────────────────────────────────────

fn tokenize(input: &str) -> Result<Vec<Token>, RustBashError> {
    let bytes = input.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        // Skip whitespace
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }

        let start = i;

        // Numbers: decimal, hex (0x/0X), octal (0...) or base#value
        if bytes[i].is_ascii_digit() {
            let num = parse_number(bytes, &mut i)?;
            // Reject floating-point literals (bash does not support them)
            if i < bytes.len() && bytes[i] == b'.' {
                return Err(RustBashError::Execution(
                    "arithmetic: syntax error: invalid arithmetic operator".into(),
                ));
            }
            // Check for base#value syntax: e.g. 16#ff, 2#101
            if i < bytes.len() && bytes[i] == b'#' {
                let base = num;
                i += 1; // skip '#'
                let val_start = i;
                while i < bytes.len()
                    && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'@' || bytes[i] == b'_')
                {
                    i += 1;
                }
                let val_str = std::str::from_utf8(&bytes[val_start..i]).unwrap();
                let result = parse_base_n_value(base, val_str)?;
                tokens.push(Token {
                    kind: TokenKind::Number(result),
                    start,
                    len: i - start,
                });
            } else {
                tokens.push(Token {
                    kind: TokenKind::Number(num),
                    start,
                    len: i - start,
                });
            }
            continue;
        }

        // Single-quoted strings: bash rejects them in arithmetic context.
        // (Associative array keys are handled by extract_raw_subscript.)
        if bytes[i] == b'\'' {
            return Err(RustBashError::Execution(
                "arithmetic: syntax error: operand expected".into(),
            ));
        }

        // Double-quoted strings: evaluate content as sub-expression
        if bytes[i] == b'"' {
            i += 1;
            let inner_start = i;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < bytes.len() {
                    i += 2;
                } else {
                    i += 1;
                }
            }
            let inner = std::str::from_utf8(&bytes[inner_start..i]).unwrap_or("");
            if i < bytes.len() {
                i += 1; // skip closing quote
            }
            // Recursively tokenize the inner content
            let inner_tokens = tokenize(inner)?;
            tokens.extend(inner_tokens);
            continue;
        }

        // Identifiers (variable names)
        if bytes[i] == b'_' || bytes[i].is_ascii_alphabetic() {
            while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                i += 1;
            }
            tokens.push(Token {
                kind: TokenKind::Ident,
                start,
                len: i - start,
            });
            continue;
        }

        // Skip $ prefix before variable names
        if bytes[i] == b'$' {
            // $VAR or ${VAR} inside arithmetic — just skip the $
            i += 1;
            if i < bytes.len() && bytes[i] == b'{' {
                // ${VAR} — skip { and find }
                i += 1;
                let var_start = i;
                while i < bytes.len() && bytes[i] != b'}' {
                    i += 1;
                }
                if i < bytes.len() {
                    let var_len = i - var_start;
                    tokens.push(Token {
                        kind: TokenKind::Ident,
                        start: var_start,
                        len: var_len,
                    });
                    i += 1; // skip }
                }
            } else if i < bytes.len() && bytes[i].is_ascii_digit() {
                // $0, $1, ..., $9 — positional parameter, emit as Ident
                let var_start = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Ident,
                    start: var_start,
                    len: i - var_start,
                });
            } else if i < bytes.len() && (bytes[i] == b'#' || bytes[i] == b'?') {
                // $# (param count), $? (last exit code)
                tokens.push(Token {
                    kind: TokenKind::Ident,
                    start: i,
                    len: 1,
                });
                i += 1;
            } else if i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphabetic()) {
                let var_start = i;
                while i < bytes.len() && (bytes[i] == b'_' || bytes[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                tokens.push(Token {
                    kind: TokenKind::Ident,
                    start: var_start,
                    len: i - var_start,
                });
            }
            continue;
        }

        // Two-character and one-character operators
        let next = if i + 1 < bytes.len() {
            Some(bytes[i + 1])
        } else {
            None
        };
        let next2 = if i + 2 < bytes.len() {
            Some(bytes[i + 2])
        } else {
            None
        };

        match (bytes[i], next, next2) {
            // Three-character operators
            (b'*', Some(b'*'), _) => {
                tokens.push(Token {
                    kind: TokenKind::StarStar,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'<', Some(b'<'), Some(b'=')) => {
                tokens.push(Token {
                    kind: TokenKind::LtLtEq,
                    start,
                    len: 3,
                });
                i += 3;
            }
            (b'>', Some(b'>'), Some(b'=')) => {
                tokens.push(Token {
                    kind: TokenKind::GtGtEq,
                    start,
                    len: 3,
                });
                i += 3;
            }
            // Two-character operators
            (b'+', Some(b'+'), _) => {
                tokens.push(Token {
                    kind: TokenKind::PlusPlus,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'-', Some(b'-'), _) => {
                tokens.push(Token {
                    kind: TokenKind::MinusMinus,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'+', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::PlusEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'-', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::MinusEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'*', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::StarEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'/', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::SlashEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'%', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::PercentEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'&', Some(b'&'), _) => {
                tokens.push(Token {
                    kind: TokenKind::AmpAmp,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'&', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::AmpEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'|', Some(b'|'), _) => {
                tokens.push(Token {
                    kind: TokenKind::PipePipe,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'|', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::PipeEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'^', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::CaretEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'=', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::EqEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'!', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::BangEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'<', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::LtEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'<', Some(b'<'), _) => {
                tokens.push(Token {
                    kind: TokenKind::LtLt,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'>', Some(b'='), _) => {
                tokens.push(Token {
                    kind: TokenKind::GtEq,
                    start,
                    len: 2,
                });
                i += 2;
            }
            (b'>', Some(b'>'), _) => {
                tokens.push(Token {
                    kind: TokenKind::GtGt,
                    start,
                    len: 2,
                });
                i += 2;
            }
            // Single-character operators
            (b'+', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Plus,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'-', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Minus,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'*', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Star,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'/', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Slash,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'%', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Percent,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'&', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Amp,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'|', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Pipe,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'^', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Caret,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'~', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Tilde,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'!', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Bang,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'=', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Eq,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'<', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Lt,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'>', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Gt,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'?', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Question,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b':', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Colon,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'(', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::LParen,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b')', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::RParen,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b'[', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::LBracket,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b']', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::RBracket,
                    start,
                    len: 1,
                });
                i += 1;
            }
            (b',', _, _) => {
                tokens.push(Token {
                    kind: TokenKind::Comma,
                    start,
                    len: 1,
                });
                i += 1;
            }
            _ => {
                return Err(RustBashError::Execution(format!(
                    "arithmetic: unexpected character `{}`",
                    bytes[i] as char
                )));
            }
        }
    }

    Ok(tokens)
}

fn parse_number(bytes: &[u8], i: &mut usize) -> Result<i64, RustBashError> {
    let start = *i;

    // Hex: 0x or 0X
    if bytes[start] == b'0'
        && *i + 1 < bytes.len()
        && (bytes[*i + 1] == b'x' || bytes[*i + 1] == b'X')
    {
        *i += 2;
        let hex_start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_hexdigit() {
            *i += 1;
        }
        if *i == hex_start {
            return Err(RustBashError::Execution(
                "arithmetic: invalid hex number".into(),
            ));
        }
        let s = std::str::from_utf8(&bytes[hex_start..*i]).unwrap();
        return i64::from_str_radix(s, 16).map_err(|_| {
            RustBashError::Execution(format!("arithmetic: invalid hex number `0x{s}`"))
        });
    }

    // Octal: leading 0 followed by digits
    if bytes[start] == b'0' && *i + 1 < bytes.len() && bytes[*i + 1].is_ascii_digit() {
        *i += 1;
        let oct_start = *i;
        while *i < bytes.len() && bytes[*i].is_ascii_digit() {
            *i += 1;
        }
        let s = std::str::from_utf8(&bytes[oct_start..*i]).unwrap();
        return i64::from_str_radix(s, 8).map_err(|_| {
            RustBashError::Execution(format!("arithmetic: invalid octal number `0{s}`"))
        });
    }

    // Decimal
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    let s = std::str::from_utf8(&bytes[start..*i]).unwrap();
    s.parse::<i64>()
        .map_err(|_| RustBashError::Execution(format!("arithmetic: invalid number `{s}`")))
}

/// Parse a value in base N (2..64). Digits: 0-9, a-z, A-Z, @, _
fn parse_base_n_value(base: i64, digits: &str) -> Result<i64, RustBashError> {
    if !(2..=64).contains(&base) {
        return Err(RustBashError::Execution(format!(
            "arithmetic: invalid arithmetic base: {base}"
        )));
    }
    let base_u = base as u64;
    let mut result: i64 = 0;
    for ch in digits.chars() {
        let digit_val = match ch {
            '0'..='9' => (ch as u64) - (b'0' as u64),
            'a'..='z' => (ch as u64) - (b'a' as u64) + 10,
            'A'..='Z' => {
                if base <= 36 {
                    (ch as u64) - (b'A' as u64) + 10
                } else {
                    (ch as u64) - (b'A' as u64) + 36
                }
            }
            '@' => 62,
            '_' => 63,
            _ => {
                return Err(RustBashError::Execution(format!(
                    "arithmetic: value too great for base: {digits} (base {base})"
                )));
            }
        };
        if digit_val >= base_u {
            return Err(RustBashError::Execution(format!(
                "arithmetic: value too great for base: {digits} (base {base})"
            )));
        }
        result = result.wrapping_mul(base).wrapping_add(digit_val as i64);
    }
    Ok(result)
}

// ── Recursive-descent parser / evaluator ────────────────────────────

struct Parser<'a> {
    tokens: &'a [Token],
    source: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn with_source(tokens: &'a [Token], source: &'a str) -> Self {
        Self {
            tokens,
            source,
            pos: 0,
        }
    }

    fn peek(&self) -> Option<TokenKind> {
        self.tokens.get(self.pos).map(|t| t.kind)
    }

    fn advance(&mut self) -> Token {
        let t = self.tokens[self.pos];
        self.pos += 1;
        t
    }

    fn expect(&mut self, kind: TokenKind) -> Result<Token, RustBashError> {
        match self.peek() {
            Some(k) if k == kind => Ok(self.advance()),
            _ => Err(RustBashError::Execution(format!(
                "arithmetic: expected {kind:?}"
            ))),
        }
    }

    fn ident_name(&self, tok: Token) -> &'a str {
        &self.source[tok.start..tok.start + tok.len]
    }

    // ── Precedence levels (low to high) ─────────────────────────────

    // Comma (lowest)
    fn parse_comma(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut result = self.parse_assignment(state)?;
        while self.peek() == Some(TokenKind::Comma) {
            self.advance();
            result = self.parse_assignment(state)?;
        }
        Ok(result)
    }

    // Assignment: = += -= *= /= %= <<= >>= &= |= ^=
    // Right-to-left associative
    fn parse_assignment(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        // Look ahead: if current is Ident followed by assignment op, handle it.
        // Also handle Ident[expr] = ... for array element assignment.
        if let Some(TokenKind::Ident) = self.peek() {
            let saved = self.pos;
            let ident_tok = self.advance();
            let name = self.ident_name(ident_tok).to_string();

            // Check for array subscript — capture raw text between [ and ]
            let raw_subscript = if self.peek() == Some(TokenKind::LBracket) {
                Some(self.extract_raw_subscript()?)
            } else {
                None
            };

            if let Some(op) = self.peek() {
                match op {
                    TokenKind::Eq => {
                        self.advance();
                        let val = self.parse_assignment(state)?;
                        if let Some(ref sub) = raw_subscript {
                            write_array_element(state, &name, sub, val)?;
                        } else {
                            set_variable(state, &name, val.to_string())?;
                        }
                        return Ok(val);
                    }
                    TokenKind::PlusEq
                    | TokenKind::MinusEq
                    | TokenKind::StarEq
                    | TokenKind::SlashEq
                    | TokenKind::PercentEq
                    | TokenKind::LtLtEq
                    | TokenKind::GtGtEq
                    | TokenKind::AmpEq
                    | TokenKind::PipeEq
                    | TokenKind::CaretEq => {
                        self.advance();
                        let rhs = self.parse_assignment(state)?;
                        let lhs = if let Some(ref sub) = raw_subscript {
                            read_array_element(state, &name, sub)?
                        } else {
                            read_var(state, &name)?
                        };
                        let val = apply_compound_op(op, lhs, rhs)?;
                        if let Some(ref sub) = raw_subscript {
                            write_array_element(state, &name, sub, val)?;
                        } else {
                            set_variable(state, &name, val.to_string())?;
                        }
                        return Ok(val);
                    }
                    _ => {
                        // Not an assignment — backtrack
                        self.pos = saved;
                    }
                }
            } else {
                self.pos = saved;
            }
        }
        self.parse_ternary(state)
    }

    // Ternary: cond ? true_val : false_val
    // Bash short-circuits: only the taken branch is evaluated for side effects.
    fn parse_ternary(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let cond = self.parse_logical_or(state)?;
        if self.peek() == Some(TokenKind::Question) {
            self.advance();
            if cond != 0 {
                let true_val = self.parse_assignment(state)?;
                self.expect(TokenKind::Colon)?;
                self.skip_ternary_branch()?;
                Ok(true_val)
            } else {
                self.skip_ternary_branch()?;
                self.expect(TokenKind::Colon)?;
                let false_val = self.parse_assignment(state)?;
                Ok(false_val)
            }
        } else {
            Ok(cond)
        }
    }

    /// Skip tokens for one ternary branch without evaluating side effects.
    /// Handles nested ternaries by tracking `?`/`:` depth.
    fn skip_ternary_branch(&mut self) -> Result<(), RustBashError> {
        let mut depth = 0;
        loop {
            match self.peek() {
                Some(TokenKind::Question) => {
                    depth += 1;
                    self.advance();
                }
                Some(TokenKind::Colon) if depth == 0 => break,
                Some(TokenKind::Colon) => {
                    depth -= 1;
                    self.advance();
                }
                None => break,
                _ => {
                    self.advance();
                }
            }
        }
        Ok(())
    }

    // Logical OR: || (short-circuit: skip RHS if LHS is truthy)
    fn parse_logical_or(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_logical_and(state)?;
        while self.peek() == Some(TokenKind::PipePipe) {
            self.advance();
            if left != 0 {
                // RHS of || is a logical-and-level expression; skip past && chains
                self.skip_logical_operand(true)?;
                left = 1;
            } else {
                let right = self.parse_logical_and(state)?;
                left = i64::from(right != 0);
            }
        }
        Ok(left)
    }

    // Logical AND: && (short-circuit: skip RHS if LHS is falsy)
    fn parse_logical_and(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_bitwise_or(state)?;
        while self.peek() == Some(TokenKind::AmpAmp) {
            self.advance();
            if left == 0 {
                // RHS of && is a bitwise-or-level expression; stop at &&
                self.skip_logical_operand(false)?;
                left = 0;
            } else {
                let right = self.parse_bitwise_or(state)?;
                left = i64::from(right != 0);
            }
        }
        Ok(left)
    }

    /// Skip one operand expression without evaluating side effects.
    /// When `skip_and` is true (called from `||`), skips past `&&` chains
    /// since `&&` has higher precedence than `||`.
    fn skip_logical_operand(&mut self, skip_and: bool) -> Result<(), RustBashError> {
        let mut paren_depth = 0i32;
        let mut bracket_depth = 0i32;
        loop {
            match self.peek() {
                None => break,
                Some(TokenKind::LParen) => {
                    paren_depth += 1;
                    self.advance();
                }
                Some(TokenKind::RParen) => {
                    if paren_depth <= 0 {
                        break;
                    }
                    paren_depth -= 1;
                    self.advance();
                }
                Some(TokenKind::LBracket) => {
                    bracket_depth += 1;
                    self.advance();
                }
                Some(TokenKind::RBracket) => {
                    bracket_depth -= 1;
                    self.advance();
                }
                Some(TokenKind::AmpAmp) if skip_and && paren_depth == 0 && bracket_depth == 0 => {
                    // Inside ||'s RHS: consume && and skip its operand too
                    self.advance();
                    self.skip_logical_operand(false)?;
                }
                Some(
                    TokenKind::PipePipe
                    | TokenKind::AmpAmp
                    | TokenKind::Question
                    | TokenKind::Colon
                    | TokenKind::Comma,
                ) if paren_depth == 0 && bracket_depth == 0 => {
                    break;
                }
                _ => {
                    self.advance();
                }
            }
        }
        Ok(())
    }

    // Bitwise OR: |
    fn parse_bitwise_or(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_bitwise_xor(state)?;
        while self.peek() == Some(TokenKind::Pipe) {
            self.advance();
            let right = self.parse_bitwise_xor(state)?;
            left |= right;
        }
        Ok(left)
    }

    // Bitwise XOR: ^
    fn parse_bitwise_xor(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_bitwise_and(state)?;
        while self.peek() == Some(TokenKind::Caret) {
            self.advance();
            let right = self.parse_bitwise_and(state)?;
            left ^= right;
        }
        Ok(left)
    }

    // Bitwise AND: &
    fn parse_bitwise_and(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_equality(state)?;
        while self.peek() == Some(TokenKind::Amp) {
            self.advance();
            let right = self.parse_equality(state)?;
            left &= right;
        }
        Ok(left)
    }

    // Equality: == !=
    fn parse_equality(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_comparison(state)?;
        loop {
            match self.peek() {
                Some(TokenKind::EqEq) => {
                    self.advance();
                    let right = self.parse_comparison(state)?;
                    left = i64::from(left == right);
                }
                Some(TokenKind::BangEq) => {
                    self.advance();
                    let right = self.parse_comparison(state)?;
                    left = i64::from(left != right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Comparison: < > <= >=
    fn parse_comparison(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_shift(state)?;
        loop {
            match self.peek() {
                Some(TokenKind::Lt) => {
                    self.advance();
                    let right = self.parse_shift(state)?;
                    left = i64::from(left < right);
                }
                Some(TokenKind::Gt) => {
                    self.advance();
                    let right = self.parse_shift(state)?;
                    left = i64::from(left > right);
                }
                Some(TokenKind::LtEq) => {
                    self.advance();
                    let right = self.parse_shift(state)?;
                    left = i64::from(left <= right);
                }
                Some(TokenKind::GtEq) => {
                    self.advance();
                    let right = self.parse_shift(state)?;
                    left = i64::from(left >= right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Shift: << >>
    fn parse_shift(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_additive(state)?;
        loop {
            match self.peek() {
                Some(TokenKind::LtLt) => {
                    self.advance();
                    let right = self.parse_additive(state)?;
                    left = left.wrapping_shl(right as u32);
                }
                Some(TokenKind::GtGt) => {
                    self.advance();
                    let right = self.parse_additive(state)?;
                    left = left.wrapping_shr(right as u32);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Addition / subtraction: + -
    fn parse_additive(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_multiplicative(state)?;
        loop {
            match self.peek() {
                Some(TokenKind::Plus) => {
                    self.advance();
                    let right = self.parse_multiplicative(state)?;
                    left = left.wrapping_add(right);
                }
                Some(TokenKind::Minus) => {
                    self.advance();
                    let right = self.parse_multiplicative(state)?;
                    left = left.wrapping_sub(right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Multiplication / division / modulo: * / %
    fn parse_multiplicative(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_exponentiation(state)?;
        loop {
            match self.peek() {
                Some(TokenKind::Star) => {
                    self.advance();
                    let right = self.parse_exponentiation(state)?;
                    left = left.wrapping_mul(right);
                }
                Some(TokenKind::Slash) => {
                    self.advance();
                    let right = self.parse_exponentiation(state)?;
                    if right == 0 {
                        return Err(RustBashError::Execution(
                            "arithmetic: division by zero".into(),
                        ));
                    }
                    left = left.wrapping_div(right);
                }
                Some(TokenKind::Percent) => {
                    self.advance();
                    let right = self.parse_exponentiation(state)?;
                    if right == 0 {
                        return Err(RustBashError::Execution(
                            "arithmetic: division by zero".into(),
                        ));
                    }
                    left = left.wrapping_rem(right);
                }
                _ => break,
            }
        }
        Ok(left)
    }

    // Exponentiation: ** (right-to-left associative)
    fn parse_exponentiation(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let base = self.parse_unary(state)?;
        if self.peek() == Some(TokenKind::StarStar) {
            self.advance();
            let exp = self.parse_exponentiation(state)?; // right-associative
            wrapping_pow(base, exp)
        } else {
            Ok(base)
        }
    }

    // Unary: + - ! ~ (right-to-left)
    fn parse_unary(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        match self.peek() {
            Some(TokenKind::Plus) => {
                self.advance();
                self.parse_unary(state)
            }
            Some(TokenKind::Minus) => {
                self.advance();
                let val = self.parse_unary(state)?;
                Ok(val.wrapping_neg())
            }
            Some(TokenKind::Bang) => {
                self.advance();
                let val = self.parse_unary(state)?;
                Ok(i64::from(val == 0))
            }
            Some(TokenKind::Tilde) => {
                self.advance();
                let val = self.parse_unary(state)?;
                Ok(!val)
            }
            // Pre-increment / pre-decrement (supports both var and var[subscript])
            Some(TokenKind::PlusPlus) => {
                self.advance();
                let tok = self.expect_ident()?;
                let name = self.ident_name(tok).to_string();
                if self.peek() == Some(TokenKind::LBracket) {
                    let raw_sub = self.extract_raw_subscript()?;
                    let old = read_array_element(state, &name, &raw_sub)?;
                    let val = old.wrapping_add(1);
                    write_array_element(state, &name, &raw_sub, val)?;
                    Ok(val)
                } else {
                    let val = read_var(state, &name)?.wrapping_add(1);
                    set_variable(state, &name, val.to_string())?;
                    Ok(val)
                }
            }
            Some(TokenKind::MinusMinus) => {
                self.advance();
                let tok = self.expect_ident()?;
                let name = self.ident_name(tok).to_string();
                if self.peek() == Some(TokenKind::LBracket) {
                    let raw_sub = self.extract_raw_subscript()?;
                    let old = read_array_element(state, &name, &raw_sub)?;
                    let val = old.wrapping_sub(1);
                    write_array_element(state, &name, &raw_sub, val)?;
                    Ok(val)
                } else {
                    let val = read_var(state, &name)?.wrapping_sub(1);
                    set_variable(state, &name, val.to_string())?;
                    Ok(val)
                }
            }
            _ => self.parse_postfix(state),
        }
    }

    // Postfix: var++ var-- (also supports var[subscript]++ and var[subscript]--)
    fn parse_postfix(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let val = self.parse_primary(state)?;

        // Check for postfix ++ or -- after an identifier (with optional subscript)
        if self.pos >= 1 {
            // Check if the previous token was ] (array subscript) or Ident (simple var)
            let prev = self.tokens[self.pos - 1];
            let is_array = matches!(prev.kind, TokenKind::RBracket);
            let is_simple_ident = matches!(prev.kind, TokenKind::Ident);

            if is_array {
                // Find the variable name and subscript by walking back
                if let Some(op @ (TokenKind::PlusPlus | TokenKind::MinusMinus)) = self.peek() {
                    self.advance();
                    // Reconstruct the var name and subscript from the parsed tokens
                    // We need to find the Ident before the [ ... ] sequence
                    if let Some((name, raw_sub)) = self.find_preceding_array_ref() {
                        let delta: i64 = if op == TokenKind::PlusPlus { 1 } else { -1 };
                        write_array_element(state, &name, &raw_sub, val.wrapping_add(delta))?;
                        return Ok(val);
                    }
                }
            } else if is_simple_ident {
                match self.peek() {
                    Some(TokenKind::PlusPlus) => {
                        self.advance();
                        let name = self.ident_name(prev).to_string();
                        set_variable(state, &name, (val.wrapping_add(1)).to_string())?;
                        return Ok(val); // return old value
                    }
                    Some(TokenKind::MinusMinus) => {
                        self.advance();
                        let name = self.ident_name(prev).to_string();
                        set_variable(state, &name, (val.wrapping_sub(1)).to_string())?;
                        return Ok(val); // return old value
                    }
                    _ => {}
                }
            }
        }
        Ok(val)
    }

    /// Walk backward from current position to find the array name and subscript
    /// text for a `name[subscript]` that was just parsed.
    fn find_preceding_array_ref(&self) -> Option<(String, String)> {
        // We expect tokens ending: Ident LBracket <subscript tokens...> RBracket
        // Walk backward from current pos - 1 (which is the postfix op we just consumed)
        // The token before that was RBracket. Find matching LBracket.
        let mut p = self.pos - 2; // pos after advance; -1 = op token, -2 = RBracket
        let mut depth = 1;
        while p > 0 {
            p -= 1;
            match self.tokens[p].kind {
                TokenKind::RBracket => depth += 1,
                TokenKind::LBracket => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
        }
        if depth != 0 || p == 0 {
            return None;
        }
        // Token before LBracket should be the identifier
        let ident_tok = self.tokens[p - 1];
        if !matches!(ident_tok.kind, TokenKind::Ident) {
            return None;
        }
        let name = self.ident_name(ident_tok).to_string();
        // Reconstruct subscript text
        let bracket_start = p;
        let bracket_end = self.pos - 2; // RBracket position
        let sub_text = if bracket_start + 1 < bracket_end {
            let first = &self.tokens[bracket_start + 1];
            let last = &self.tokens[bracket_end - 1];
            self.source[first.start..last.start + last.len].to_string()
        } else {
            String::from("0")
        };
        Some((name, sub_text))
    }

    // Primary: number, variable, parenthesized expression
    fn parse_primary(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        match self.peek() {
            Some(TokenKind::Number(n)) => {
                self.advance();
                Ok(n)
            }
            Some(TokenKind::Ident) => {
                let tok = self.advance();
                let name = self.ident_name(tok).to_string();
                // Check for array subscript: ident[expr]
                if self.peek() == Some(TokenKind::LBracket) {
                    let raw_sub = self.extract_raw_subscript()?;
                    // Reject double subscript: a[i][j]
                    if self.peek() == Some(TokenKind::LBracket) {
                        return Err(RustBashError::Execution(
                            "arithmetic: syntax error in expression".into(),
                        ));
                    }
                    read_array_element_checked(state, &name, &raw_sub)
                } else {
                    Ok(read_var(state, &name)?)
                }
            }
            Some(TokenKind::LParen) => {
                self.advance();
                let val = self.parse_comma(state)?;
                self.expect(TokenKind::RParen)?;
                Ok(val)
            }
            Some(kind) => Err(RustBashError::Execution(format!(
                "arithmetic: unexpected token {kind:?}"
            ))),
            None => Err(RustBashError::Execution(
                "arithmetic: unexpected end of expression".into(),
            )),
        }
    }

    /// Extract the raw source text of an array subscript between `[` and `]`.
    /// The parser position must be at the `[` token. After this call, the
    /// position is advanced past the matching `]`.
    fn extract_raw_subscript(&mut self) -> Result<String, RustBashError> {
        self.expect(TokenKind::LBracket)?;
        // Find the matching ] — track nesting
        let start_pos = self.pos;
        let mut depth = 1;
        while self.pos < self.tokens.len() {
            match self.tokens[self.pos].kind {
                TokenKind::LBracket => depth += 1,
                TokenKind::RBracket => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            self.pos += 1;
        }
        if depth != 0 {
            return Err(RustBashError::Execution(
                "arithmetic: expected RBracket".into(),
            ));
        }
        // Reconstruct the raw source text between [ and ]
        let raw = if start_pos < self.pos {
            let first = &self.tokens[start_pos];
            let last = &self.tokens[self.pos - 1];
            let src_start = first.start;
            let src_end = last.start + last.len;
            self.source[src_start..src_end].to_string()
        } else {
            String::new()
        };
        self.advance(); // consume ]
        Ok(raw)
    }

    fn expect_ident(&mut self) -> Result<Token, RustBashError> {
        match self.peek() {
            Some(TokenKind::Ident) => Ok(self.advance()),
            _ => Err(RustBashError::Execution(
                "arithmetic: expected variable name".into(),
            )),
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn read_var(state: &mut InterpreterState, name: &str) -> Result<i64, RustBashError> {
    // Handle special parameters
    match name {
        "#" => return Ok(state.positional_params.len() as i64),
        "?" => return Ok(state.last_exit_code as i64),
        "LINENO" => return Ok(state.current_lineno as i64),
        "SECONDS" => return Ok(state.shell_start_time.elapsed().as_secs() as i64),
        _ => {}
    }
    // Handle positional parameters ($0, $1, $2, ...)
    if let Ok(n) = name.parse::<usize>() {
        if n == 0 {
            return Ok(state.shell_name.parse::<i64>().unwrap_or(0));
        }
        return Ok(state
            .positional_params
            .get(n - 1)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0));
    }
    // Check nounset before resolving
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    if state.shell_opts.nounset && !state.env.contains_key(&resolved) {
        return Err(RustBashError::Execution(format!(
            "{name}: unbound variable"
        )));
    }
    Ok(resolve_var_recursive(state, name, 0))
}

fn resolve_var_recursive(state: &mut InterpreterState, name: &str, depth: usize) -> i64 {
    const MAX_DEPTH: usize = 10;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    let val_str = state
        .env
        .get(&resolved)
        .map(|v| v.value.as_scalar().to_string())
        .unwrap_or_default();
    if val_str.is_empty() {
        return 0;
    }
    if let Ok(n) = val_str.parse::<i64>() {
        return n;
    }
    // If the value looks like a valid variable name, resolve recursively.
    if depth < MAX_DEPTH
        && val_str
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !val_str.chars().next().unwrap_or('0').is_ascii_digit()
    {
        return resolve_var_recursive(state, &val_str, depth + 1);
    }
    // Bash evaluates the variable's string value as an arithmetic expression.
    if depth < MAX_DEPTH
        && let Ok(n) = eval_arithmetic(&val_str, state)
    {
        return n;
    }
    0
}

/// Strip surrounding single or double quotes from an associative array key.
fn strip_assoc_quotes(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('\'') && s.ends_with('\'')) || (s.starts_with('"') && s.ends_with('"')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Determine if a variable is an associative array.
fn is_assoc_array(state: &InterpreterState, name: &str) -> bool {
    use crate::interpreter::VariableValue;
    let resolved = crate::interpreter::resolve_nameref_or_self(name, state);
    state
        .env
        .get(&resolved)
        .is_some_and(|v| matches!(v.value, VariableValue::AssociativeArray(_)))
}

/// Read a specific array element.
/// For associative arrays, the raw subscript is used as a string key.
/// For indexed arrays, it is evaluated as an arithmetic expression.
/// Checks nounset if enabled.
fn read_array_element(
    state: &mut InterpreterState,
    name: &str,
    raw_subscript: &str,
) -> Result<i64, RustBashError> {
    use crate::interpreter::VariableValue;
    let resolved_name = crate::interpreter::resolve_nameref_or_self(name, state);

    // Determine type and extract value without holding a borrow across eval_arithmetic.
    enum VarKind {
        Assoc,
        Indexed,
        Scalar,
        Missing,
    }
    let kind = match state.env.get(&resolved_name) {
        Some(v) => match &v.value {
            VariableValue::AssociativeArray(_) => VarKind::Assoc,
            VariableValue::IndexedArray(_) => VarKind::Indexed,
            VariableValue::Scalar(_) => VarKind::Scalar,
        },
        None => VarKind::Missing,
    };

    let val_str = match kind {
        VarKind::Missing => return Ok(0),
        VarKind::Assoc => {
            let key = strip_assoc_quotes(raw_subscript);
            match state.env.get(&resolved_name) {
                Some(v) => match &v.value {
                    VariableValue::AssociativeArray(map) => {
                        map.get(key).cloned().unwrap_or_default()
                    }
                    _ => String::new(),
                },
                None => String::new(),
            }
        }
        VarKind::Indexed => {
            let index = eval_arithmetic(raw_subscript, state).unwrap_or(0);
            match state.env.get(&resolved_name) {
                Some(v) => match &v.value {
                    VariableValue::IndexedArray(map) => {
                        let actual_idx = if index < 0 {
                            let max_key = map.keys().next_back().copied().unwrap_or(0);
                            let resolved = max_key as i64 + 1 + index;
                            if resolved < 0 {
                                return Ok(0);
                            }
                            resolved as usize
                        } else {
                            index as usize
                        };
                        map.get(&actual_idx).cloned().unwrap_or_default()
                    }
                    _ => String::new(),
                },
                None => String::new(),
            }
        }
        VarKind::Scalar => {
            let index = eval_arithmetic(raw_subscript, state).unwrap_or(0);
            match state.env.get(&resolved_name) {
                Some(v) => match &v.value {
                    VariableValue::Scalar(s) => {
                        if index == 0 || index == -1 {
                            s.clone()
                        } else {
                            String::new()
                        }
                    }
                    _ => String::new(),
                },
                None => String::new(),
            }
        }
    };
    if val_str.is_empty() {
        return Ok(0);
    }
    match val_str.parse::<i64>() {
        Ok(v) => Ok(v),
        Err(_) => {
            // Guard against infinite recursion (e.g. a[0]="a[0]").
            use std::cell::Cell;
            thread_local! {
                static DEPTH: Cell<usize> = const { Cell::new(0) };
            }
            DEPTH.with(|d| {
                let cur = d.get();
                if cur >= 10 {
                    return Err(RustBashError::Execution(format!(
                        "{name}[{raw_subscript}]: recursive evaluation depth exceeded"
                    )));
                }
                d.set(cur + 1);
                let result = eval_arithmetic(&val_str, state);
                d.set(cur);
                result
            })
        }
    }
}

/// Like `read_array_element`, but returns a `Result` to propagate nounset errors.
fn read_array_element_checked(
    state: &mut InterpreterState,
    name: &str,
    raw_subscript: &str,
) -> Result<i64, RustBashError> {
    let resolved_name = crate::interpreter::resolve_nameref_or_self(name, state);
    if state.shell_opts.nounset && !state.env.contains_key(&resolved_name) {
        return Err(RustBashError::Execution(format!(
            "{name}[{raw_subscript}]: unbound variable"
        )));
    }
    read_array_element(state, name, raw_subscript)
}

/// Write a value to a specific array element.
/// For associative arrays, the raw subscript is used as a string key.
/// For indexed arrays, it is evaluated as an arithmetic expression.
fn write_array_element(
    state: &mut InterpreterState,
    name: &str,
    raw_subscript: &str,
    value: i64,
) -> Result<(), RustBashError> {
    use crate::interpreter::VariableValue;
    let resolved_name = crate::interpreter::resolve_nameref_or_self(name, state);
    if is_assoc_array(state, &resolved_name) {
        let key = strip_assoc_quotes(raw_subscript).to_string();
        return crate::interpreter::set_assoc_element(
            state,
            &resolved_name,
            key,
            value.to_string(),
        );
    }
    let index = eval_arithmetic(raw_subscript, state)?;
    if index < 0 {
        let max_key = state.env.get(&resolved_name).and_then(|v| match &v.value {
            VariableValue::IndexedArray(map) => map.keys().next_back().copied(),
            VariableValue::Scalar(_) => Some(0),
            _ => None,
        });
        match max_key {
            Some(mk) => {
                let resolved = mk as i64 + 1 + index;
                if resolved < 0 {
                    return Err(RustBashError::Execution(format!(
                        "{name}: bad array subscript"
                    )));
                }
                return crate::interpreter::set_array_element(
                    state,
                    &resolved_name,
                    resolved as usize,
                    value.to_string(),
                );
            }
            None => {
                return Err(RustBashError::Execution(format!(
                    "{name}: bad array subscript"
                )));
            }
        }
    }
    crate::interpreter::set_array_element(state, &resolved_name, index as usize, value.to_string())
}

fn wrapping_pow(mut base: i64, mut exp: i64) -> Result<i64, RustBashError> {
    if exp < 0 {
        return Err(RustBashError::Execution(
            "arithmetic: exponent less than 0".into(),
        ));
    }
    let mut result: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result.wrapping_mul(base);
        }
        exp >>= 1;
        base = base.wrapping_mul(base);
    }
    Ok(result)
}

fn apply_compound_op(op: TokenKind, lhs: i64, rhs: i64) -> Result<i64, RustBashError> {
    match op {
        TokenKind::PlusEq => Ok(lhs.wrapping_add(rhs)),
        TokenKind::MinusEq => Ok(lhs.wrapping_sub(rhs)),
        TokenKind::StarEq => Ok(lhs.wrapping_mul(rhs)),
        TokenKind::SlashEq => {
            if rhs == 0 {
                return Err(RustBashError::Execution(
                    "arithmetic: division by zero".into(),
                ));
            }
            Ok(lhs.wrapping_div(rhs))
        }
        TokenKind::PercentEq => {
            if rhs == 0 {
                return Err(RustBashError::Execution(
                    "arithmetic: division by zero".into(),
                ));
            }
            Ok(lhs.wrapping_rem(rhs))
        }
        TokenKind::LtLtEq => Ok(lhs.wrapping_shl(rhs as u32)),
        TokenKind::GtGtEq => Ok(lhs.wrapping_shr(rhs as u32)),
        TokenKind::AmpEq => Ok(lhs & rhs),
        TokenKind::PipeEq => Ok(lhs | rhs),
        TokenKind::CaretEq => Ok(lhs ^ rhs),
        _ => unreachable!(),
    }
}

// ── Unit tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpreter::{
        ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts, ShoptOpts,
    };
    use crate::network::NetworkPolicy;
    use crate::vfs::InMemoryFs;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn make_state() -> InterpreterState {
        InterpreterState {
            fs: Arc::new(InMemoryFs::new()),
            env: HashMap::new(),
            cwd: "/".to_string(),
            functions: HashMap::new(),
            last_exit_code: 0,
            commands: HashMap::new(),
            shell_opts: ShellOpts::default(),
            shopt_opts: ShoptOpts::default(),
            limits: ExecutionLimits::default(),
            counters: ExecutionCounters::default(),
            network_policy: NetworkPolicy::default(),
            should_exit: false,
            loop_depth: 0,
            control_flow: None,
            positional_params: Vec::new(),
            shell_name: "rust-bash".to_string(),
            random_seed: 42,
            local_scopes: Vec::new(),
            in_function_depth: 0,
            traps: HashMap::new(),
            in_trap: false,
            errexit_suppressed: 0,
            stdin_offset: 0,
            dir_stack: Vec::new(),
            command_hash: HashMap::new(),
            aliases: HashMap::new(),
            current_lineno: 0,
            shell_start_time: crate::platform::Instant::now(),
            last_argument: String::new(),
            call_stack: Vec::new(),
            machtype: "x86_64-pc-linux-gnu".to_string(),
            hosttype: "x86_64".to_string(),
            persistent_fds: HashMap::new(),
            next_auto_fd: 10,
            proc_sub_counter: 0,
            proc_sub_prealloc: HashMap::new(),
            pipe_stdin_bytes: None,
            pending_cmdsub_stderr: String::new(),
        }
    }

    fn eval(expr: &str) -> i64 {
        let mut state = make_state();
        eval_arithmetic(expr, &mut state).unwrap()
    }

    fn eval_with(expr: &str, state: &mut InterpreterState) -> i64 {
        eval_arithmetic(expr, state).unwrap()
    }

    #[test]
    fn basic_addition() {
        assert_eq!(eval("1 + 2"), 3);
    }

    #[test]
    fn multiplication() {
        assert_eq!(eval("5 * 3"), 15);
    }

    #[test]
    fn division() {
        assert_eq!(eval("10 / 3"), 3);
    }

    #[test]
    fn modulo() {
        assert_eq!(eval("10 % 3"), 1);
    }

    #[test]
    fn exponentiation() {
        assert_eq!(eval("2 ** 10"), 1024);
    }

    #[test]
    fn precedence_add_mul() {
        assert_eq!(eval("2 + 3 * 4"), 14);
    }

    #[test]
    fn parenthesized() {
        assert_eq!(eval("(1 + 2) * 3"), 9);
    }

    #[test]
    fn comparison_gt() {
        assert_eq!(eval("5 > 3"), 1);
    }

    #[test]
    fn comparison_lt() {
        assert_eq!(eval("5 < 3"), 0);
    }

    #[test]
    fn comparison_le() {
        assert_eq!(eval("3 <= 3"), 1);
    }

    #[test]
    fn comparison_ge() {
        assert_eq!(eval("3 >= 4"), 0);
    }

    #[test]
    fn equality() {
        assert_eq!(eval("5 == 5"), 1);
        assert_eq!(eval("5 != 5"), 0);
        assert_eq!(eval("5 != 3"), 1);
    }

    #[test]
    fn logical_and() {
        assert_eq!(eval("1 && 0"), 0);
        assert_eq!(eval("1 && 1"), 1);
    }

    #[test]
    fn logical_or() {
        assert_eq!(eval("1 || 0"), 1);
        assert_eq!(eval("0 || 0"), 0);
    }

    #[test]
    fn bitwise_and() {
        assert_eq!(eval("0xFF & 0x0F"), 15);
    }

    #[test]
    fn bitwise_or() {
        assert_eq!(eval("0xF0 | 0x0F"), 255);
    }

    #[test]
    fn bitwise_xor() {
        assert_eq!(eval("0xFF ^ 0x0F"), 240);
    }

    #[test]
    fn bitwise_shift() {
        assert_eq!(eval("1 << 8"), 256);
        assert_eq!(eval("256 >> 4"), 16);
    }

    #[test]
    fn ternary() {
        assert_eq!(eval("5 > 3 ? 10 : 20"), 10);
        assert_eq!(eval("5 < 3 ? 10 : 20"), 20);
    }

    #[test]
    fn unary_minus() {
        assert_eq!(eval("-5"), -5);
    }

    #[test]
    fn unary_plus() {
        assert_eq!(eval("+5"), 5);
    }

    #[test]
    fn bitwise_not() {
        assert_eq!(eval("~0"), -1);
    }

    #[test]
    fn logical_not() {
        assert_eq!(eval("! 0"), 1);
        assert_eq!(eval("! 1"), 0);
    }

    #[test]
    fn hex_literal() {
        assert_eq!(eval("0xFF"), 255);
    }

    #[test]
    fn octal_literal() {
        assert_eq!(eval("077"), 63);
    }

    #[test]
    fn variable_read() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x + 3", &mut state), 8);
    }

    #[test]
    fn variable_with_dollar() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("$x + 3", &mut state), 8);
    }

    #[test]
    fn variable_assignment() {
        let mut state = make_state();
        let result = eval_with("x = 5", &mut state);
        assert_eq!(result, 5);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "5");
    }

    #[test]
    fn compound_assignment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "10".into()).unwrap();
        assert_eq!(eval_with("x += 5", &mut state), 15);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "15");
    }

    #[test]
    fn pre_increment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("++x", &mut state), 6);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "6");
    }

    #[test]
    fn post_increment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x++", &mut state), 5);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "6");
    }

    #[test]
    fn pre_decrement() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("--x", &mut state), 4);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "4");
    }

    #[test]
    fn post_decrement() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x--", &mut state), 5);
        assert_eq!(state.env.get("x").unwrap().value.as_scalar(), "4");
    }

    #[test]
    fn division_by_zero() {
        let mut state = make_state();
        assert!(eval_arithmetic("1 / 0", &mut state).is_err());
    }

    #[test]
    fn modulo_by_zero() {
        let mut state = make_state();
        assert!(eval_arithmetic("1 % 0", &mut state).is_err());
    }

    #[test]
    fn undefined_variable_defaults_to_zero() {
        assert_eq!(eval("undefined_var"), 0);
    }

    #[test]
    fn empty_expression() {
        assert_eq!(eval(""), 0);
    }

    #[test]
    fn nested_parens() {
        assert_eq!(eval("((2 + 3) * (4 - 1))"), 15);
    }

    #[test]
    fn comma_operator() {
        let mut state = make_state();
        let result = eval_with("x = 1, y = 2, x + y", &mut state);
        assert_eq!(result, 3);
    }

    #[test]
    fn complex_expression() {
        assert_eq!(eval("2 + 3 * 4 - 1"), 13);
    }

    #[test]
    fn dollar_brace_variable() {
        let mut state = make_state();
        set_variable(&mut state, "foo", "42".into()).unwrap();
        assert_eq!(eval_with("${foo} + 1", &mut state), 43);
    }
}
