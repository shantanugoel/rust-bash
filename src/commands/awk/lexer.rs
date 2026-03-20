use std::fmt;

/// Token types produced by the awk lexer.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // Literals
    Number(f64),
    StringLit(String),
    Regex(String),
    Ident(String),

    // Keywords
    Begin,
    End,
    If,
    Else,
    While,
    For,
    Do,
    Break,
    Continue,
    Next,
    Exit,
    In,
    Delete,
    Getline,
    Print,
    Printf,

    // Operators
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Caret,
    Assign,
    PlusAssign,
    MinusAssign,
    StarAssign,
    SlashAssign,
    PercentAssign,
    CaretAssign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Match,    // ~
    NotMatch, // !~
    And,      // &&
    Or,       // ||
    Not,      // !
    Increment,
    Decrement,
    Dollar, // $ (field reference)

    // Punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Semicolon,
    Comma,
    Question,
    Colon,
    Newline,

    // Special
    Append, // >> (for output redirect, parsed but not fully supported)
    Pipe,   // | (for output redirect, parsed but not fully supported)

    Eof,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Number(n) => write!(f, "{n}"),
            Token::StringLit(s) => write!(f, "\"{s}\""),
            Token::Regex(r) => write!(f, "/{r}/"),
            Token::Ident(s) => write!(f, "{s}"),
            Token::Begin => write!(f, "BEGIN"),
            Token::End => write!(f, "END"),
            Token::If => write!(f, "if"),
            Token::Else => write!(f, "else"),
            Token::While => write!(f, "while"),
            Token::For => write!(f, "for"),
            Token::Do => write!(f, "do"),
            Token::Break => write!(f, "break"),
            Token::Continue => write!(f, "continue"),
            Token::Next => write!(f, "next"),
            Token::Exit => write!(f, "exit"),
            Token::In => write!(f, "in"),
            Token::Delete => write!(f, "delete"),
            Token::Getline => write!(f, "getline"),
            Token::Print => write!(f, "print"),
            Token::Printf => write!(f, "printf"),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Percent => write!(f, "%"),
            Token::Caret => write!(f, "^"),
            Token::Assign => write!(f, "="),
            Token::PlusAssign => write!(f, "+="),
            Token::MinusAssign => write!(f, "-="),
            Token::StarAssign => write!(f, "*="),
            Token::SlashAssign => write!(f, "/="),
            Token::PercentAssign => write!(f, "%="),
            Token::CaretAssign => write!(f, "^="),
            Token::Eq => write!(f, "=="),
            Token::Ne => write!(f, "!="),
            Token::Lt => write!(f, "<"),
            Token::Le => write!(f, "<="),
            Token::Gt => write!(f, ">"),
            Token::Ge => write!(f, ">="),
            Token::Match => write!(f, "~"),
            Token::NotMatch => write!(f, "!~"),
            Token::And => write!(f, "&&"),
            Token::Or => write!(f, "||"),
            Token::Not => write!(f, "!"),
            Token::Increment => write!(f, "++"),
            Token::Decrement => write!(f, "--"),
            Token::Dollar => write!(f, "$"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::Semicolon => write!(f, ";"),
            Token::Comma => write!(f, ","),
            Token::Question => write!(f, "?"),
            Token::Colon => write!(f, ":"),
            Token::Newline => write!(f, "\\n"),
            Token::Append => write!(f, ">>"),
            Token::Pipe => write!(f, "|"),
            Token::Eof => write!(f, "EOF"),
        }
    }
}

pub struct Lexer {
    input: Vec<char>,
    pos: usize,
    tokens: Vec<Token>,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Self {
            input: input.chars().collect(),
            pos: 0,
            tokens: Vec::new(),
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, String> {
        while self.pos < self.input.len() {
            self.skip_whitespace_and_comments();
            if self.pos >= self.input.len() {
                break;
            }

            let ch = self.input[self.pos];
            match ch {
                '\n' => {
                    self.tokens.push(Token::Newline);
                    self.pos += 1;
                }
                '\\' if self.peek_char(1) == Some('\n') => {
                    // Line continuation
                    self.pos += 2;
                }
                '"' => self.lex_string()?,
                '0'..='9' | '.' if self.is_start_of_number() => self.lex_number()?,
                'a'..='z' | 'A'..='Z' | '_' => self.lex_ident(),
                '$' => {
                    self.tokens.push(Token::Dollar);
                    self.pos += 1;
                }
                '+' => {
                    if self.peek_char(1) == Some('+') {
                        self.tokens.push(Token::Increment);
                        self.pos += 2;
                    } else if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::PlusAssign);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Plus);
                        self.pos += 1;
                    }
                }
                '-' => {
                    if self.peek_char(1) == Some('-') {
                        self.tokens.push(Token::Decrement);
                        self.pos += 2;
                    } else if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::MinusAssign);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Minus);
                        self.pos += 1;
                    }
                }
                '*' => {
                    if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::StarAssign);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Star);
                        self.pos += 1;
                    }
                }
                '/' => {
                    if self.should_lex_regex() {
                        self.lex_regex()?;
                    } else if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::SlashAssign);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Slash);
                        self.pos += 1;
                    }
                }
                '%' => {
                    if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::PercentAssign);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Percent);
                        self.pos += 1;
                    }
                }
                '^' => {
                    if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::CaretAssign);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Caret);
                        self.pos += 1;
                    }
                }
                '=' => {
                    if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::Eq);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Assign);
                        self.pos += 1;
                    }
                }
                '!' => {
                    if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::Ne);
                        self.pos += 2;
                    } else if self.peek_char(1) == Some('~') {
                        self.tokens.push(Token::NotMatch);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Not);
                        self.pos += 1;
                    }
                }
                '<' => {
                    if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::Le);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Lt);
                        self.pos += 1;
                    }
                }
                '>' => {
                    if self.peek_char(1) == Some('=') {
                        self.tokens.push(Token::Ge);
                        self.pos += 2;
                    } else if self.peek_char(1) == Some('>') {
                        self.tokens.push(Token::Append);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Gt);
                        self.pos += 1;
                    }
                }
                '~' => {
                    self.tokens.push(Token::Match);
                    self.pos += 1;
                }
                '&' => {
                    if self.peek_char(1) == Some('&') {
                        self.tokens.push(Token::And);
                        self.pos += 2;
                    } else {
                        return Err(format!("unexpected character '&' at position {}", self.pos));
                    }
                }
                '|' => {
                    if self.peek_char(1) == Some('|') {
                        self.tokens.push(Token::Or);
                        self.pos += 2;
                    } else {
                        self.tokens.push(Token::Pipe);
                        self.pos += 1;
                    }
                }
                '(' => {
                    self.tokens.push(Token::LParen);
                    self.pos += 1;
                }
                ')' => {
                    self.tokens.push(Token::RParen);
                    self.pos += 1;
                }
                '{' => {
                    self.tokens.push(Token::LBrace);
                    self.pos += 1;
                }
                '}' => {
                    self.tokens.push(Token::RBrace);
                    self.pos += 1;
                }
                '[' => {
                    self.tokens.push(Token::LBracket);
                    self.pos += 1;
                }
                ']' => {
                    self.tokens.push(Token::RBracket);
                    self.pos += 1;
                }
                ';' => {
                    self.tokens.push(Token::Semicolon);
                    self.pos += 1;
                }
                ',' => {
                    self.tokens.push(Token::Comma);
                    self.pos += 1;
                }
                '?' => {
                    self.tokens.push(Token::Question);
                    self.pos += 1;
                }
                ':' => {
                    self.tokens.push(Token::Colon);
                    self.pos += 1;
                }
                _ => {
                    return Err(format!(
                        "unexpected character '{ch}' at position {}",
                        self.pos
                    ));
                }
            }
        }
        self.tokens.push(Token::Eof);
        Ok(self.tokens)
    }

    fn peek_char(&self, offset: usize) -> Option<char> {
        self.input.get(self.pos + offset).copied()
    }

    fn is_start_of_number(&self) -> bool {
        let ch = self.input[self.pos];
        if ch.is_ascii_digit() {
            return true;
        }
        // '.' is a number start only if followed by a digit
        if ch == '.'
            && let Some(&next) = self.input.get(self.pos + 1)
        {
            return next.is_ascii_digit();
        }
        false
    }

    fn skip_whitespace_and_comments(&mut self) {
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == ' ' || ch == '\t' || ch == '\r' {
                self.pos += 1;
            } else if ch == '#' {
                // Comment until end of line
                while self.pos < self.input.len() && self.input[self.pos] != '\n' {
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    fn lex_string(&mut self) -> Result<(), String> {
        self.pos += 1; // skip opening quote
        let mut s = String::new();
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == '"' {
                self.pos += 1;
                self.tokens.push(Token::StringLit(s));
                return Ok(());
            } else if ch == '\\' {
                self.pos += 1;
                if self.pos >= self.input.len() {
                    return Err("unterminated string escape".to_string());
                }
                let esc = self.input[self.pos];
                match esc {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    'r' => s.push('\r'),
                    '\\' => s.push('\\'),
                    '"' => s.push('"'),
                    'a' => s.push('\x07'),
                    'b' => s.push('\x08'),
                    'f' => s.push('\x0c'),
                    'v' => s.push('\x0b'),
                    '/' => s.push('/'),
                    _ => {
                        s.push('\\');
                        s.push(esc);
                    }
                }
                self.pos += 1;
            } else {
                s.push(ch);
                self.pos += 1;
            }
        }
        Err("unterminated string literal".to_string())
    }

    fn lex_number(&mut self) -> Result<(), String> {
        let start = self.pos;
        // Handle hex: 0x...
        if self.input[self.pos] == '0'
            && self.pos + 1 < self.input.len()
            && (self.input[self.pos + 1] == 'x' || self.input[self.pos + 1] == 'X')
        {
            self.pos += 2;
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_hexdigit() {
                self.pos += 1;
            }
            let hex_str: String = self.input[start..self.pos].iter().collect();
            let val = i64::from_str_radix(&hex_str[2..], 16)
                .map_err(|e| format!("invalid hex number: {e}"))?;
            self.tokens.push(Token::Number(val as f64));
            return Ok(());
        }

        while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        if self.pos < self.input.len() && self.input[self.pos] == '.' {
            self.pos += 1;
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        // Scientific notation
        if self.pos < self.input.len()
            && (self.input[self.pos] == 'e' || self.input[self.pos] == 'E')
        {
            self.pos += 1;
            if self.pos < self.input.len()
                && (self.input[self.pos] == '+' || self.input[self.pos] == '-')
            {
                self.pos += 1;
            }
            while self.pos < self.input.len() && self.input[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let num_str: String = self.input[start..self.pos].iter().collect();
        let val: f64 = num_str
            .parse()
            .map_err(|e| format!("invalid number '{num_str}': {e}"))?;
        self.tokens.push(Token::Number(val));
        Ok(())
    }

    fn lex_ident(&mut self) {
        let start = self.pos;
        while self.pos < self.input.len()
            && (self.input[self.pos].is_alphanumeric() || self.input[self.pos] == '_')
        {
            self.pos += 1;
        }
        let word: String = self.input[start..self.pos].iter().collect();
        let token = match word.as_str() {
            "BEGIN" => Token::Begin,
            "END" => Token::End,
            "if" => Token::If,
            "else" => Token::Else,
            "while" => Token::While,
            "for" => Token::For,
            "do" => Token::Do,
            "break" => Token::Break,
            "continue" => Token::Continue,
            "next" => Token::Next,
            "exit" => Token::Exit,
            "in" => Token::In,
            "delete" => Token::Delete,
            "getline" => Token::Getline,
            "print" => Token::Print,
            "printf" => Token::Printf,
            _ => Token::Ident(word),
        };
        self.tokens.push(token);
    }

    fn lex_regex(&mut self) -> Result<(), String> {
        self.pos += 1; // skip opening /
        let mut pattern = String::new();
        while self.pos < self.input.len() {
            let ch = self.input[self.pos];
            if ch == '/' {
                self.pos += 1;
                self.tokens.push(Token::Regex(pattern));
                return Ok(());
            } else if ch == '\\' {
                pattern.push('\\');
                self.pos += 1;
                if self.pos < self.input.len() {
                    pattern.push(self.input[self.pos]);
                    self.pos += 1;
                }
            } else if ch == '\n' {
                return Err("unterminated regex literal".to_string());
            } else {
                pattern.push(ch);
                self.pos += 1;
            }
        }
        Err("unterminated regex literal".to_string())
    }

    /// Determine if `/` starts a regex or is a division operator.
    /// A regex follows: start of input, operator, keyword, punctuation (except `)` and `]`),
    /// or a value token separated by at least one newline (rule boundary).
    fn should_lex_regex(&self) -> bool {
        let mut saw_newline = false;
        let prev = self.tokens.iter().rev().find(|t| {
            if matches!(t, Token::Newline) {
                saw_newline = true;
                false
            } else {
                true
            }
        });
        match prev {
            None => true, // start of input
            Some(t) => {
                if matches!(
                    t,
                    Token::Semicolon
                        | Token::LBrace
                        | Token::RBrace
                        | Token::LParen
                        | Token::Comma
                        | Token::Not
                        | Token::And
                        | Token::Or
                        | Token::Match
                        | Token::NotMatch
                        | Token::Assign
                        | Token::PlusAssign
                        | Token::MinusAssign
                        | Token::StarAssign
                        | Token::SlashAssign
                        | Token::PercentAssign
                        | Token::CaretAssign
                        | Token::Eq
                        | Token::Ne
                        | Token::Lt
                        | Token::Le
                        | Token::Gt
                        | Token::Ge
                        | Token::Question
                        | Token::Colon
                        | Token::Print
                        | Token::Printf
                        | Token::Begin
                        | Token::End
                        | Token::If
                        | Token::While
                        | Token::For
                        | Token::Do
                ) {
                    return true;
                }
                // At rule boundaries (newline between value token and /), treat as regex
                saw_newline
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_simple_print() {
        let tokens = Lexer::new("{print $1}").tokenize().unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::LBrace,
                Token::Print,
                Token::Dollar,
                Token::Number(1.0),
                Token::RBrace,
                Token::Eof,
            ]
        );
    }

    #[test]
    fn tokenize_begin_end() {
        let tokens = Lexer::new("BEGIN{x=0} {x++} END{print x}")
            .tokenize()
            .unwrap();
        assert!(matches!(tokens[0], Token::Begin));
        assert!(tokens.iter().any(|t| matches!(t, Token::End)));
    }

    #[test]
    fn tokenize_regex() {
        let tokens = Lexer::new("/error/ {print}").tokenize().unwrap();
        assert_eq!(tokens[0], Token::Regex("error".to_string()));
    }

    #[test]
    fn tokenize_string_escapes() {
        let tokens = Lexer::new(r#""hello\nworld""#).tokenize().unwrap();
        assert_eq!(tokens[0], Token::StringLit("hello\nworld".to_string()));
    }

    #[test]
    fn tokenize_comparison_ops() {
        let tokens = Lexer::new("$1 >= 10 && $2 != \"\"").tokenize().unwrap();
        assert!(tokens.contains(&Token::Ge));
        assert!(tokens.contains(&Token::And));
        assert!(tokens.contains(&Token::Ne));
    }
}
