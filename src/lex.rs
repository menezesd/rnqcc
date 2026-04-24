use crate::types::Token;

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        Lexer {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_ahead(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while let Some(c) = self.peek() {
                if c.is_whitespace() {
                    self.advance();
                } else {
                    break;
                }
            }
            // Skip line comments
            if self.peek() == Some('/') && self.peek_ahead(1) == Some('/') {
                self.advance();
                self.advance();
                while let Some(c) = self.peek() {
                    self.advance();
                    if c == '\n' {
                        break;
                    }
                }
                continue;
            }
            // Skip block comments
            if self.peek() == Some('/') && self.peek_ahead(1) == Some('*') {
                self.advance();
                self.advance();
                loop {
                    match self.advance() {
                        Some('*') if self.peek() == Some('/') => {
                            self.advance();
                            break;
                        }
                        None => panic!("Unterminated block comment"),
                        _ => {}
                    }
                }
                continue;
            }
            break;
        }
    }

    fn read_number(&mut self) -> Token {
        let start = self.pos;

        // Handle leading '.' for floats like .5
        let mut is_float = false;
        if self.peek() == Some('.') {
            is_float = true;
            self.advance();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() { self.advance(); } else { break; }
            }
            if matches!(self.peek(), Some('e' | 'E')) {
                self.advance();
                if matches!(self.peek(), Some('+' | '-')) { self.advance(); }
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() { self.advance(); } else { break; }
                }
            }
            let num_str: String = self.chars[start..self.pos].iter().collect();
            let value = num_str.parse::<f64>().unwrap();
            if matches!(self.peek(), Some('f' | 'F' | 'l' | 'L')) { self.advance(); }
            return Token::DoubleLiteral(value);
        }

        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                self.advance();
            } else {
                break;
            }
        }

        // Check if this is a floating-point number
        if self.peek() == Some('.') {
            is_float = true;
            self.advance(); // consume '.'
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() { self.advance(); } else { break; }
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            self.advance(); // consume 'e'/'E'
            if matches!(self.peek(), Some('+' | '-')) {
                self.advance(); // consume sign
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() { self.advance(); } else { break; }
            }
        }

        if is_float {
            let num_str: String = self.chars[start..self.pos].iter().collect();
            let value = num_str.parse::<f64>()
                .unwrap_or_else(|_| panic!("Invalid float literal: {}", num_str));
            // Consume optional f/F/l/L suffix (all treated as double)
            if matches!(self.peek(), Some('f' | 'F' | 'l' | 'L')) {
                self.advance();
            }
            if let Some(c) = self.peek() {
                if c.is_ascii_alphabetic() || c == '_' {
                    panic!("Invalid float literal suffix at position {}", self.pos);
                }
            }
            return Token::DoubleLiteral(value);
        }

        let num_end = self.pos;

        // Check for suffixes: u/U, l/L, ul/UL, lu/LU (case insensitive)
        let mut is_long = false;
        let mut is_unsigned = false;
        for _ in 0..2 {
            match self.peek() {
                Some('L' | 'l') if !is_long => {
                    self.advance();
                    is_long = true;
                }
                Some('U' | 'u') if !is_unsigned => {
                    self.advance();
                    is_unsigned = true;
                }
                _ => break,
            }
        }

        // Check that the number is not immediately followed by an identifier char
        if let Some(c) = self.peek() {
            if c.is_ascii_alphabetic() || c == '_' {
                panic!(
                    "Invalid number literal at position {}: digit followed by '{}'",
                    start, c
                );
            }
        }
        let num_str: String = self.chars[start..num_end].iter().collect();
        // Parse as u64 first to handle large unsigned constants, then transmute to i64
        let value = if is_unsigned {
            num_str.parse::<u64>()
                .unwrap_or_else(|_| panic!("Invalid integer literal: {}", num_str))
                as i64
        } else {
            num_str.parse::<i64>()
                .or_else(|_| num_str.parse::<u64>().map(|v| v as i64))
                .unwrap_or_else(|_| panic!("Invalid integer literal: {}", num_str))
        };
        match (is_unsigned, is_long) {
            (true, true) => Token::ULongLiteral(value),
            (true, false) => Token::UIntLiteral(value),
            (false, true) => Token::LongLiteral(value),
            (false, false) => Token::IntLiteral(value),
        }
    }

    fn unescape_char(&mut self) -> char {
        match self.advance() {
            Some('\\') => match self.advance() {
                Some('n') => '\n',
                Some('t') => '\t',
                Some('r') => '\r',
                Some('\\') => '\\',
                Some('\'') => '\'',
                Some('"') => '"',
                Some('?') => '?',
                Some('a') => '\x07',
                Some('b') => '\x08',
                Some('f') => '\x0C',
                Some('v') => '\x0B',
                Some('0') => '\0',
                Some(c) => panic!("Unknown escape sequence: \\{}", c),
                None => panic!("Unexpected end of input in escape sequence"),
            },
            Some(c) => c,
            None => panic!("Unexpected end of input in character/string literal"),
        }
    }

    fn read_char_constant(&mut self) -> Token {
        // Opening ' already consumed
        let c = self.unescape_char();
        match self.advance() {
            Some('\'') => {}
            _ => panic!("Expected closing single quote"),
        }
        Token::CharLiteral(c as i64)
    }

    fn read_string_literal(&mut self) -> Token {
        // Opening " already consumed
        let mut s = String::new();
        loop {
            match self.peek() {
                Some('"') => { self.advance(); break; }
                Some('\n') | None => panic!("Unterminated string literal"),
                _ => {
                    let c = self.unescape_char();
                    s.push(c);
                }
            }
        }
        Token::StringLiteral(s)
    }

    fn read_identifier_or_keyword(&mut self) -> Token {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_alphanumeric() || c == '_' {
                self.advance();
            } else {
                break;
            }
        }
        let word: String = self.chars[start..self.pos].iter().collect();
        match word.as_str() {
            "int" => Token::KWInt,
            "long" => Token::KWLong,
            "unsigned" => Token::KWUnsigned,
            "signed" => Token::KWSigned,
            "double" => Token::KWDouble,
            "float" => Token::KWFloat,
            "void" => Token::KWVoid,
            "return" => Token::KWReturn,
            "if" => Token::KWIf,
            "else" => Token::KWElse,
            "while" => Token::KWWhile,
            "for" => Token::KWFor,
            "do" => Token::KWDo,
            "break" => Token::KWBreak,
            "continue" => Token::KWContinue,
            "goto" => Token::KWGoto,
            "switch" => Token::KWSwitch,
            "case" => Token::KWCase,
            "default" => Token::KWDefault,
            "static" => Token::KWStatic,
            "extern" => Token::KWExtern,
            "char" => Token::KWChar,
            _ => Token::Identifier(word),
        }
    }

    /// Try to match a second character; if it matches, consume it and return `yes`,
    /// otherwise return `no`.
    fn two_char(&mut self, expected: char, yes: Token, no: Token) -> Token {
        if self.peek() == Some(expected) {
            self.advance();
            yes
        } else {
            no
        }
    }

    pub fn lex_all(&mut self) -> Vec<Token> {
        let mut tokens = Vec::new();

        loop {
            self.skip_whitespace_and_comments();
            let c = match self.advance() {
                Some(c) => c,
                None => break,
            };

            let tok = match c {
                '(' => Token::OpenParen,
                ')' => Token::CloseParen,
                '{' => Token::OpenBrace,
                '}' => Token::CloseBrace,
                ';' => Token::Semicolon,
                ',' => Token::Comma,
                '[' => Token::OpenBracket,
                ']' => Token::CloseBracket,
                '~' => Token::Tilde,
                '?' => Token::Question,
                ':' => Token::Colon,

                '+' => {
                    if self.peek() == Some('+') {
                        self.advance();
                        Token::Increment
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::PlusAssign
                    } else {
                        Token::Plus
                    }
                }
                '-' => {
                    if self.peek() == Some('-') {
                        self.advance();
                        Token::Decrement
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::MinusAssign
                    } else {
                        Token::Minus
                    }
                }
                '*' => self.two_char('=', Token::StarAssign, Token::Star),
                '/' => self.two_char('=', Token::SlashAssign, Token::Slash),
                '%' => self.two_char('=', Token::PercentAssign, Token::Percent),

                '&' => {
                    if self.peek() == Some('&') {
                        self.advance();
                        Token::LogicalAnd
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::AmpersandAssign
                    } else {
                        Token::Ampersand
                    }
                }
                '|' => {
                    if self.peek() == Some('|') {
                        self.advance();
                        Token::LogicalOr
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::PipeAssign
                    } else {
                        Token::Pipe
                    }
                }
                '^' => self.two_char('=', Token::CaretAssign, Token::Caret),

                '<' => {
                    if self.peek() == Some('<') {
                        self.advance();
                        self.two_char('=', Token::ShiftLeftAssign, Token::ShiftLeft)
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::LessEqual
                    } else {
                        Token::LessThan
                    }
                }
                '>' => {
                    if self.peek() == Some('>') {
                        self.advance();
                        self.two_char('=', Token::ShiftRightAssign, Token::ShiftRight)
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::GreaterEqual
                    } else {
                        Token::GreaterThan
                    }
                }

                '=' => self.two_char('=', Token::EqualEqual, Token::Assign),
                '!' => self.two_char('=', Token::NotEqual, Token::Bang),

                '\'' => self.read_char_constant(),
                '"' => self.read_string_literal(),

                // Float literal starting with '.' (e.g., .5)
                '.' if self.peek().map_or(false, |c| c.is_ascii_digit()) => {
                    self.pos -= 1; // unget the '.'
                    self.read_number()
                }
                _ if c.is_ascii_digit() => {
                    self.pos -= 1; // unget
                    self.read_number()
                }
                _ if c.is_ascii_alphabetic() || c == '_' => {
                    self.pos -= 1; // unget
                    self.read_identifier_or_keyword()
                }

                _ => panic!("Unexpected character '{}' at position {}", c, self.pos - 1),
            };

            tokens.push(tok);
        }

        tokens
    }
}

pub fn lex(input: &str) -> Vec<Token> {
    let mut lexer = Lexer::new(input);
    lexer.lex_all()
}
