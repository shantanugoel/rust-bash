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

        // Numbers: decimal, hex (0x/0X), octal (0...)
        if bytes[i].is_ascii_digit() {
            let num = parse_number(bytes, &mut i)?;
            tokens.push(Token {
                kind: TokenKind::Number(num),
                start,
                len: i - start,
            });
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
        // Otherwise fall through to ternary.
        if let Some(TokenKind::Ident) = self.peek() {
            let saved = self.pos;
            let ident_tok = self.advance();
            let name = self.ident_name(ident_tok).to_string();

            if let Some(op) = self.peek() {
                match op {
                    TokenKind::Eq => {
                        self.advance();
                        let val = self.parse_assignment(state)?;
                        set_variable(state, &name, val.to_string())?;
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
                        let lhs = read_var(state, &name);
                        let val = apply_compound_op(op, lhs, rhs)?;
                        set_variable(state, &name, val.to_string())?;
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
    fn parse_ternary(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let cond = self.parse_logical_or(state)?;
        if self.peek() == Some(TokenKind::Question) {
            self.advance();
            let true_val = self.parse_assignment(state)?;
            self.expect(TokenKind::Colon)?;
            let false_val = self.parse_assignment(state)?;
            Ok(if cond != 0 { true_val } else { false_val })
        } else {
            Ok(cond)
        }
    }

    // Logical OR: ||
    fn parse_logical_or(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_logical_and(state)?;
        while self.peek() == Some(TokenKind::PipePipe) {
            self.advance();
            let right = self.parse_logical_and(state)?;
            left = i64::from(left != 0 || right != 0);
        }
        Ok(left)
    }

    // Logical AND: &&
    fn parse_logical_and(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let mut left = self.parse_bitwise_or(state)?;
        while self.peek() == Some(TokenKind::AmpAmp) {
            self.advance();
            let right = self.parse_bitwise_or(state)?;
            left = i64::from(left != 0 && right != 0);
        }
        Ok(left)
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
            Ok(wrapping_pow(base, exp))
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
            // Pre-increment / pre-decrement
            Some(TokenKind::PlusPlus) => {
                self.advance();
                let tok = self.expect_ident()?;
                let name = self.ident_name(tok).to_string();
                let val = read_var(state, &name).wrapping_add(1);
                set_variable(state, &name, val.to_string())?;
                Ok(val)
            }
            Some(TokenKind::MinusMinus) => {
                self.advance();
                let tok = self.expect_ident()?;
                let name = self.ident_name(tok).to_string();
                let val = read_var(state, &name).wrapping_sub(1);
                set_variable(state, &name, val.to_string())?;
                Ok(val)
            }
            _ => self.parse_postfix(state),
        }
    }

    // Postfix: var++ var--
    fn parse_postfix(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        let val = self.parse_primary(state)?;

        // Check for postfix ++ or -- (only valid after an identifier)
        if self.pos >= 1 {
            let prev = self.tokens[self.pos - 1];
            if let TokenKind::Ident = prev.kind {
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

    // Primary: number, variable, parenthesized expression
    fn parse_primary(&mut self, state: &mut InterpreterState) -> Result<i64, RustBashError> {
        match self.peek() {
            Some(TokenKind::Number(n)) => {
                self.advance();
                Ok(n)
            }
            Some(TokenKind::Ident) => {
                let tok = self.advance();
                let name = self.ident_name(tok);
                Ok(read_var(state, name))
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

fn read_var(state: &InterpreterState, name: &str) -> i64 {
    // Handle special parameters
    match name {
        "#" => return state.positional_params.len() as i64,
        "?" => return state.last_exit_code as i64,
        _ => {}
    }
    // Handle positional parameters ($0, $1, $2, ...)
    if let Ok(n) = name.parse::<usize>() {
        if n == 0 {
            return state.shell_name.parse::<i64>().unwrap_or(0);
        }
        return state
            .positional_params
            .get(n - 1)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
    }
    state
        .env
        .get(name)
        .map(|v| v.value.parse::<i64>().unwrap_or(0))
        .unwrap_or(0)
}

fn wrapping_pow(mut base: i64, mut exp: i64) -> i64 {
    if exp < 0 {
        return 0; // bash treats negative exponents as 0
    }
    let mut result: i64 = 1;
    while exp > 0 {
        if exp & 1 == 1 {
            result = result.wrapping_mul(base);
        }
        exp >>= 1;
        base = base.wrapping_mul(base);
    }
    result
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
    use crate::interpreter::{ExecutionCounters, ExecutionLimits, InterpreterState, ShellOpts};
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
        assert_eq!(state.env.get("x").unwrap().value, "5");
    }

    #[test]
    fn compound_assignment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "10".into()).unwrap();
        assert_eq!(eval_with("x += 5", &mut state), 15);
        assert_eq!(state.env.get("x").unwrap().value, "15");
    }

    #[test]
    fn pre_increment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("++x", &mut state), 6);
        assert_eq!(state.env.get("x").unwrap().value, "6");
    }

    #[test]
    fn post_increment() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x++", &mut state), 5);
        assert_eq!(state.env.get("x").unwrap().value, "6");
    }

    #[test]
    fn pre_decrement() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("--x", &mut state), 4);
        assert_eq!(state.env.get("x").unwrap().value, "4");
    }

    #[test]
    fn post_decrement() {
        let mut state = make_state();
        set_variable(&mut state, "x", "5".into()).unwrap();
        assert_eq!(eval_with("x--", &mut state), 5);
        assert_eq!(state.env.get("x").unwrap().value, "4");
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
