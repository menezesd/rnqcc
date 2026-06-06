use crate::types::{
    universal_character_escape_error, universal_character_name_error,
    validate_universal_character_value, Token,
};
use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLocation {
    pub file: Option<String>,
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpan {
    pub start: SourceLocation,
    pub end: SourceLocation,
    pub start_offset: usize,
    pub end_offset: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpannedToken {
    pub token: Token,
    pub span: SourceSpan,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLineMapping {
    pub file: Option<String>,
    pub line: usize,
}

pub struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line_starts: Vec<usize>,
    line_map: Option<Vec<SourceLineMapping>>,
    pending_tokens: VecDeque<Token>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharLiteralPrefix {
    None,
    Wide,
    Utf8,
    Utf16,
    Utf32,
}

struct FloatSuffixes {
    imaginary: bool,
    long_double: bool,
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_start(ch)
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_continue(ch)
}

fn hex_value(ch: char) -> Option<u32> {
    ch.to_digit(16)
}

impl Lexer {
    pub fn new(input: &str) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let line_starts = Self::line_starts_for(&chars);
        Lexer {
            chars,
            pos: 0,
            line_starts,
            line_map: None,
            pending_tokens: VecDeque::new(),
        }
    }

    pub fn with_line_map(input: &str, line_map: Vec<SourceLineMapping>) -> Self {
        let chars: Vec<char> = input.chars().collect();
        let line_starts = Self::line_starts_for(&chars);
        Lexer {
            chars,
            pos: 0,
            line_starts,
            line_map: Some(line_map),
            pending_tokens: VecDeque::new(),
        }
    }

    fn line_starts_for(chars: &[char]) -> Vec<usize> {
        let mut starts = vec![0];
        for (idx, ch) in chars.iter().enumerate() {
            if *ch == '\n' {
                starts.push(idx + 1);
            }
        }
        starts
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_ahead(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn peek_ucn_at(&self, pos: usize) -> Option<(char, usize)> {
        self.peek_ucn_at_result(pos).ok().flatten()
    }

    fn peek_ucn_at_result(&self, pos: usize) -> Result<Option<(char, usize)>, String> {
        if self.chars.get(pos) != Some(&'\\') {
            return Ok(None);
        }
        let (digits, len) = match self.chars.get(pos + 1).copied() {
            Some('u') => (4, 6),
            Some('U') => (8, 10),
            Some(_) => return Ok(None),
            None => return Ok(None),
        };
        let mut value = 0u32;
        for offset in 0..digits {
            let Some(ch) = self.chars.get(pos + 2 + offset).copied() else {
                return Err(universal_character_name_error("incomplete spelling"));
            };
            let Some(digit) = hex_value(ch) else {
                return Err(universal_character_name_error("malformed hex digits"));
            };
            value = (value << 4) | digit;
        }
        if let Err(reason) = validate_universal_character_value(value) {
            return Err(universal_character_name_error(reason));
        }
        char::from_u32(value)
            .map(|ch| Some((ch, len)))
            .ok_or_else(|| universal_character_name_error("out-of-range universal character"))
    }

    fn peek_ucn(&self) -> Option<(char, usize)> {
        self.peek_ucn_at(self.pos)
    }

    fn starts_ucn_at(&self, pos: usize) -> bool {
        self.chars.get(pos) == Some(&'\\') && matches!(self.chars.get(pos + 1), Some('u' | 'U'))
    }

    fn peek_ident_continue(&self) -> Option<(char, usize)> {
        if let Some(ch) = self.peek().filter(|ch| is_ident_continue(*ch)) {
            Some((ch, 1))
        } else {
            self.peek_ucn().filter(|(ch, _)| is_ident_continue(*ch))
        }
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_whitespace_and_comments(&mut self) -> Result<(), String> {
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
                        None => return Err("unterminated block comment".to_string()),
                        _ => {}
                    }
                }
                continue;
            }
            break;
        }
        Ok(())
    }

    fn parse_aligned_attribute(text: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            if chars[pos].is_ascii_alphabetic() || chars[pos] == '_' {
                let start = pos;
                pos += 1;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_')
                {
                    pos += 1;
                }
                let name: String = chars[start..pos].iter().collect();
                if !matches!(
                    name.as_str(),
                    "aligned" | "__aligned__" | "align" | "__align__"
                ) {
                    continue;
                }
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if chars.get(pos) != Some(&'(') {
                    continue;
                }
                pos += 1;
                let mut depth = 1;
                let mut expression = String::new();
                while pos < chars.len() {
                    match chars[pos] {
                        '(' => {
                            depth += 1;
                            expression.push(chars[pos]);
                        }
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            expression.push(chars[pos]);
                        }
                        c => expression.push(c),
                    }
                    pos += 1;
                }
                let expression = expression.trim();
                if depth == 0 && !expression.is_empty() {
                    return Some(expression.to_string());
                }
            } else {
                pos += 1;
            }
        }
        None
    }

    fn parse_alias_attribute(text: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            if chars[pos].is_ascii_alphabetic() || chars[pos] == '_' {
                let start = pos;
                pos += 1;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_')
                {
                    pos += 1;
                }
                let name: String = chars[start..pos].iter().collect();
                if !matches!(name.as_str(), "alias" | "__alias__") {
                    continue;
                }
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if chars.get(pos) != Some(&'(') {
                    continue;
                }
                pos += 1;
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if chars.get(pos) != Some(&'"') {
                    continue;
                }
                pos += 1;
                let alias_start = pos;
                while pos < chars.len() && chars[pos] != '"' {
                    pos += 1;
                }
                if pos < chars.len() {
                    return Some(chars[alias_start..pos].iter().collect());
                }
            } else {
                pos += 1;
            }
        }
        None
    }

    fn parse_mode_attribute(text: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            if chars[pos].is_ascii_alphabetic() || chars[pos] == '_' {
                let start = pos;
                pos += 1;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_')
                {
                    pos += 1;
                }
                let name: String = chars[start..pos].iter().collect();
                if !matches!(name.as_str(), "mode" | "__mode__") {
                    continue;
                }
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if chars.get(pos) != Some(&'(') {
                    continue;
                }
                pos += 1;
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                let mode_start = pos;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_')
                {
                    pos += 1;
                }
                let mode: String = chars[mode_start..pos].iter().collect();
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if !mode.is_empty() && chars.get(pos) == Some(&')') {
                    return Some(mode);
                }
            } else {
                pos += 1;
            }
        }
        None
    }

    fn parse_vector_size_attribute(text: &str) -> Option<String> {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            if chars[pos].is_ascii_alphabetic() || chars[pos] == '_' {
                let start = pos;
                pos += 1;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_')
                {
                    pos += 1;
                }
                let name: String = chars[start..pos].iter().collect();
                if !matches!(name.as_str(), "vector_size" | "__vector_size__") {
                    continue;
                }
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if chars.get(pos) != Some(&'(') {
                    continue;
                }
                pos += 1;
                let expr_start = pos;
                let mut depth = 1;
                while pos < chars.len() {
                    match chars[pos] {
                        '(' => depth += 1,
                        ')' => {
                            depth -= 1;
                            if depth == 0 {
                                let expr: String = chars[expr_start..pos].iter().collect();
                                return Some(expr.trim().to_string());
                            }
                        }
                        _ => {}
                    }
                    pos += 1;
                }
            } else {
                pos += 1;
            }
        }
        None
    }

    fn parse_deprecated_attribute(text: &str) -> Option<Option<String>> {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            if chars[pos].is_ascii_alphabetic() || chars[pos] == '_' {
                let start = pos;
                pos += 1;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_')
                {
                    pos += 1;
                }
                let name: String = chars[start..pos].iter().collect();
                if !matches!(name.as_str(), "deprecated" | "__deprecated__") {
                    continue;
                }
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if chars.get(pos) != Some(&'(') {
                    return Some(None);
                }
                pos += 1;
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                if chars.get(pos) != Some(&'"') {
                    return Some(None);
                }
                pos += 1;
                let mut message = String::new();
                while pos < chars.len() {
                    match chars[pos] {
                        '"' => return Some(Some(message)),
                        '\\' if pos + 1 < chars.len() => {
                            pos += 1;
                            match chars[pos] {
                                'n' | 't' | 'r' => {
                                    message.push('.');
                                    message.push(chars[pos]);
                                }
                                other => message.push(other),
                            }
                        }
                        ch if ch.is_control() => message.push('.'),
                        ch => message.push(ch),
                    }
                    pos += 1;
                }
                return Some(Some(message));
            } else {
                pos += 1;
            }
        }
        None
    }

    fn contains_noreturn_attribute(text: &str) -> bool {
        Self::contains_named_attribute(text, &["noreturn", "__noreturn__"])
    }

    fn contains_no_instrument_function_attribute(text: &str) -> bool {
        Self::contains_named_attribute(
            text,
            &["no_instrument_function", "__no_instrument_function__"],
        )
    }

    fn contains_packed_attribute(text: &str) -> bool {
        Self::contains_named_attribute(text, &["packed", "__packed__"])
    }

    fn contains_transparent_union_attribute(text: &str) -> bool {
        Self::contains_named_attribute(text, &["transparent_union", "__transparent_union__"])
    }

    fn contains_reverse_scalar_storage_order_attribute(text: &str) -> bool {
        text.contains("scalar_storage_order") && text.contains("big-endian")
    }

    fn contains_named_attribute(text: &str, names: &[&str]) -> bool {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        while pos < chars.len() {
            if chars[pos].is_ascii_alphabetic() || chars[pos] == '_' {
                let start = pos;
                pos += 1;
                while pos < chars.len() && (chars[pos].is_ascii_alphanumeric() || chars[pos] == '_')
                {
                    pos += 1;
                }
                let name: String = chars[start..pos].iter().collect();
                if names.contains(&name.as_str()) {
                    return true;
                }
            } else {
                pos += 1;
            }
        }
        false
    }

    fn consume_float_suffixes(&mut self) -> FloatSuffixes {
        let mut saw_float_suffix = false;
        let mut saw_long_double_suffix = false;
        let mut saw_imaginary_suffix = false;
        loop {
            match self.peek() {
                Some('f' | 'F' | 'l' | 'L' | 'd' | 'D') if !saw_float_suffix => {
                    saw_float_suffix = true;
                    let first = self.advance();
                    if matches!(first, Some('l' | 'L')) {
                        saw_long_double_suffix = true;
                    }
                    if matches!(first, Some('d' | 'D')) && matches!(self.peek(), Some('d' | 'D')) {
                        self.advance();
                    }
                }
                Some('i' | 'I' | 'j' | 'J') if !saw_imaginary_suffix => {
                    saw_imaginary_suffix = true;
                    self.advance();
                }
                _ => break,
            }
        }
        FloatSuffixes {
            imaginary: saw_imaginary_suffix,
            long_double: saw_long_double_suffix,
        }
    }

    fn read_number(&mut self) -> Result<Token, String> {
        let start = self.pos;

        // Handle leading '.' for floats like .5
        let mut is_float = false;
        if self.peek() == Some('.') {
            self.advance();
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
            if matches!(self.peek(), Some('e' | 'E')) {
                self.advance();
                if matches!(self.peek(), Some('+' | '-')) {
                    self.advance();
                }
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        self.advance();
                    } else {
                        break;
                    }
                }
            }
            let num_str: String = self.chars[start..self.pos].iter().collect();
            let value = num_str
                .parse::<f64>()
                .map_err(|_| format!("invalid float literal: {}", num_str))?;
            let suffixes = self.consume_float_suffixes();
            if let Some(c) = self.peek() {
                if c.is_ascii_alphabetic() || c == '_' {
                    return Err(format!(
                        "invalid float literal suffix at position {}",
                        self.pos
                    ));
                }
            }
            return Ok(if suffixes.imaginary {
                Token::ImaginaryDoubleLiteral(value)
            } else if suffixes.long_double {
                Token::LongDoubleLiteral(value)
            } else {
                Token::DoubleLiteral(value)
            });
        }

        let mut radix = 10;
        let mut invalid_octal_digit = false;
        if self.peek() == Some('0') && matches!(self.peek_ahead(1), Some('x' | 'X')) {
            self.advance();
            self.advance();
            radix = 16;
            let mut value = 0.0f64;
            let digit_start = self.pos;
            while let Some(c) = self.peek() {
                if let Some(digit) = c.to_digit(16) {
                    self.advance();
                    value = value * 16.0 + f64::from(digit);
                } else {
                    break;
                }
            }
            let mut saw_digit = self.pos != digit_start;
            let mut is_hex_float = false;
            if self.peek() == Some('.') {
                is_hex_float = true;
                self.advance();
                let mut scale = 1.0f64;
                while let Some(c) = self.peek() {
                    if let Some(digit) = c.to_digit(16) {
                        self.advance();
                        scale *= 16.0;
                        value += f64::from(digit) / scale;
                        saw_digit = true;
                    } else {
                        break;
                    }
                }
            }
            if !saw_digit {
                return Err(format!("invalid hexadecimal literal at position {}", start));
            }
            if matches!(self.peek(), Some('p' | 'P')) {
                is_hex_float = true;
                self.advance();
                let exponent_sign = if self.peek() == Some('-') {
                    self.advance();
                    -1
                } else {
                    if self.peek() == Some('+') {
                        self.advance();
                    }
                    1
                };
                let exponent_start = self.pos;
                let mut exponent = 0i32;
                while let Some(c) = self.peek() {
                    if let Some(digit) = c.to_digit(10) {
                        self.advance();
                        exponent = exponent.saturating_mul(10).saturating_add(digit as i32);
                    } else {
                        break;
                    }
                }
                if self.pos == exponent_start {
                    return Err(format!(
                        "missing exponent digits in hexadecimal float literal at position {}",
                        start
                    ));
                }
                value *= 2.0f64.powi(exponent_sign * exponent);
            } else if is_hex_float {
                return Err(format!(
                    "missing binary exponent in hexadecimal float literal at position {}",
                    start
                ));
            }
            if is_hex_float {
                let suffixes = self.consume_float_suffixes();
                if let Some(c) = self.peek() {
                    if c.is_ascii_alphabetic() || c == '_' {
                        return Err(format!(
                            "invalid float literal suffix at position {}",
                            self.pos
                        ));
                    }
                }
                return Ok(if suffixes.imaginary {
                    Token::ImaginaryDoubleLiteral(value)
                } else if suffixes.long_double {
                    Token::LongDoubleLiteral(value)
                } else {
                    Token::DoubleLiteral(value)
                });
            }
        } else if self.peek() == Some('0') && matches!(self.peek_ahead(1), Some('b' | 'B')) {
            self.advance();
            self.advance();
            radix = 2;
            let digit_start = self.pos;
            while let Some(c) = self.peek() {
                if matches!(c, '0' | '1') {
                    self.advance();
                } else if c.is_ascii_digit() {
                    return Err(format!(
                        "invalid binary integer literal at position {}",
                        start
                    ));
                } else {
                    break;
                }
            }
            if self.pos == digit_start {
                return Err(format!(
                    "invalid binary integer literal at position {}",
                    start
                ));
            }
        } else if self.peek() == Some('0') {
            self.advance();
            radix = 8;
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    if !('0'..='7').contains(&c) {
                        invalid_octal_digit = true;
                    }
                    self.advance();
                } else {
                    break;
                }
            }
        } else {
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        // Check if this is a floating-point number. Leading-zero floats are
        // decimal floats, not octal integers followed by a separate suffix.
        if radix == 8 && matches!(self.peek(), Some('.' | 'e' | 'E')) {
            radix = 10;
        }
        if radix == 10 && self.peek() == Some('.') {
            is_float = true;
            self.advance(); // consume '.'
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
        }
        if radix == 10 && matches!(self.peek(), Some('e' | 'E')) {
            is_float = true;
            self.advance(); // consume 'e'/'E'
            if matches!(self.peek(), Some('+' | '-')) {
                self.advance(); // consume sign
            }
            while let Some(c) = self.peek() {
                if c.is_ascii_digit() {
                    self.advance();
                } else {
                    break;
                }
            }
        }

        if is_float {
            let num_str: String = self.chars[start..self.pos].iter().collect();
            let value = num_str
                .parse::<f64>()
                .map_err(|_| format!("invalid float literal: {}", num_str))?;
            // Consume optional floating and GNU imaginary suffixes (all treated as double).
            let suffixes = self.consume_float_suffixes();
            if let Some(c) = self.peek() {
                if c.is_ascii_alphabetic() || c == '_' {
                    return Err(format!(
                        "invalid float literal suffix at position {}",
                        self.pos
                    ));
                }
            }
            return Ok(if suffixes.imaginary {
                Token::ImaginaryDoubleLiteral(value)
            } else if suffixes.long_double {
                Token::LongDoubleLiteral(value)
            } else {
                Token::DoubleLiteral(value)
            });
        }

        let num_end = self.pos;
        if radix == 8 && invalid_octal_digit {
            return Err(format!(
                "invalid octal integer literal at position {}",
                start
            ));
        }

        // Check for suffixes: u/U, l/L, ll/LL, ul/UL, ull/ULL, lu/LU, llu/LLU.
        // rnqcc models both long and long long as the same 64-bit type.
        let mut is_long = false;
        let mut is_unsigned = false;
        loop {
            match self.peek() {
                Some('L' | 'l') if !is_long => {
                    self.advance();
                    if matches!(self.peek(), Some('L' | 'l')) {
                        self.advance();
                    }
                    is_long = true;
                }
                Some('U' | 'u') if !is_unsigned => {
                    self.advance();
                    is_unsigned = true;
                }
                _ => break,
            }
        }
        let has_imaginary_suffix = if matches!(self.peek(), Some('i' | 'I' | 'j' | 'J')) {
            self.advance();
            true
        } else {
            false
        };

        // Check that the number is not immediately followed by an identifier char
        if let Some(c) = self.peek() {
            if is_ident_start(c) || self.peek_ucn().is_some_and(|(ch, _)| is_ident_start(ch)) {
                return Err(format!(
                    "invalid number literal at position {}: digit followed by '{}'",
                    start, c
                ));
            }
        }
        let num_str: String = self.chars[start..num_end].iter().collect();
        // Parse as u128 first so GNU __int128-sized constants survive lexing.
        let digits = if radix == 16 || radix == 2 {
            &num_str[2..]
        } else {
            num_str.as_str()
        };
        let unsigned_value = u128::from_str_radix(digits, radix)
            .map_err(|_| format!("invalid integer literal: {}", num_str))?;
        if has_imaginary_suffix && unsigned_value <= i64::MAX as u128 {
            return Ok(Token::ImaginaryIntLiteral(unsigned_value as i64));
        }
        let value64 = unsigned_value as u64 as i64;
        if is_unsigned {
            if unsigned_value <= u32::MAX as u128 && !is_long {
                return Ok(Token::UIntLiteral(value64));
            }
            if unsigned_value <= u64::MAX as u128 {
                return Ok(Token::ULongLiteral(value64));
            }
            return Ok(Token::UInt128Literal(unsigned_value));
        }

        let is_decimal = radix == 10;
        if is_long {
            if unsigned_value <= i64::MAX as u128 {
                return Ok(Token::LongLiteral(value64));
            }
            if !is_decimal && unsigned_value <= u64::MAX as u128 {
                return Ok(Token::ULongLiteral(value64));
            }
            if unsigned_value <= i128::MAX as u128 {
                return Ok(Token::Int128Literal(unsigned_value as i128));
            }
            return Ok(Token::UInt128Literal(unsigned_value));
        }

        if is_decimal {
            if unsigned_value <= i64::MAX as u128 {
                return Ok(Token::IntLiteral(value64));
            }
            if unsigned_value <= i128::MAX as u128 {
                return Ok(Token::Int128Literal(unsigned_value as i128));
            }
            return Ok(Token::UInt128Literal(unsigned_value));
        }

        if unsigned_value <= i32::MAX as u128 {
            Ok(Token::IntLiteral(value64))
        } else if unsigned_value <= u32::MAX as u128 {
            Ok(Token::UIntLiteral(value64))
        } else if unsigned_value <= i64::MAX as u128 {
            Ok(Token::LongLiteral(value64))
        } else if unsigned_value <= u64::MAX as u128 {
            Ok(Token::ULongLiteral(value64))
        } else if unsigned_value <= i128::MAX as u128 {
            Ok(Token::Int128Literal(unsigned_value as i128))
        } else {
            Ok(Token::UInt128Literal(unsigned_value))
        }
    }

    fn unescape_char(&mut self) -> Result<char, String> {
        match self.advance() {
            Some('\\') => match self.advance() {
                Some('x') => {
                    let mut value = 0u32;
                    let mut digits = 0usize;
                    while let Some(c) = self.peek() {
                        let Some(digit) = c.to_digit(16) else {
                            break;
                        };
                        self.advance();
                        value = (value << 4) | digit;
                        digits += 1;
                    }
                    if digits == 0 {
                        return Err("expected hexadecimal digits after \\x escape".to_string());
                    }
                    char::from_u32(value)
                        .ok_or_else(|| "invalid hexadecimal escape value".to_string())
                }
                Some('u') => self.read_universal_character_escape(4),
                Some('U') => self.read_universal_character_escape(8),
                Some(c @ '0'..='7') => {
                    let mut value = c.to_digit(8).unwrap_or(0);
                    for _ in 0..2 {
                        match self.peek() {
                            Some(next @ '0'..='7') => {
                                self.advance();
                                value = (value << 3) | next.to_digit(8).unwrap_or(0);
                            }
                            _ => break,
                        }
                    }
                    char::from_u32(value).ok_or_else(|| "invalid octal escape value".to_string())
                }
                Some('n') => Ok('\n'),
                Some('t') => Ok('\t'),
                Some('r') => Ok('\r'),
                Some('\\') => Ok('\\'),
                Some('\'') => Ok('\''),
                Some('"') => Ok('"'),
                Some('?') => Ok('?'),
                Some('a') => Ok('\x07'),
                Some('b') => Ok('\x08'),
                Some('f') => Ok('\x0C'),
                Some('v') => Ok('\x0B'),
                Some(c) => Err(format!("unknown escape sequence: \\{}", c)),
                None => Err("unexpected end of input in escape sequence".to_string()),
            },
            Some(c) => Ok(c),
            None => Err("unexpected end of input in character/string literal".to_string()),
        }
    }

    fn read_universal_character_escape(&mut self, digits: usize) -> Result<char, String> {
        let mut value = 0u32;
        for _ in 0..digits {
            let c = self
                .advance()
                .ok_or_else(|| universal_character_escape_error("incomplete spelling"))?;
            let digit = c
                .to_digit(16)
                .ok_or_else(|| universal_character_escape_error("malformed hex digits"))?;
            value = (value << 4) | digit;
        }
        if let Err(reason) = validate_universal_character_value(value) {
            return Err(universal_character_escape_error(reason));
        }
        char::from_u32(value)
            .ok_or_else(|| universal_character_escape_error("out-of-range universal character"))
    }

    fn decode_utf8_byte_chars(chars: &[u32]) -> Option<String> {
        if chars.iter().all(|ch| *ch <= u8::MAX as u32) {
            let bytes: Vec<u8> = chars.iter().map(|ch| *ch as u8).collect();
            String::from_utf8(bytes).ok()
        } else {
            None
        }
    }

    fn read_char_constant(&mut self, prefix: CharLiteralPrefix) -> Result<Token, String> {
        // Opening ' already consumed
        let mut value = 0i64;
        let mut chars = Vec::new();
        let mut saw_char = false;
        loop {
            if self.peek() == Some('\'') {
                self.advance();
                break;
            }
            if self.peek().is_none() {
                return Err("expected closing single quote".to_string());
            }
            let c = self.unescape_char()?;
            let ch = c as u32;
            chars.push(ch);
            value = (value << 8) | i64::from(ch);
            saw_char = true;
        }
        if !saw_char {
            return Err("empty character constant".to_string());
        }
        match prefix {
            CharLiteralPrefix::Utf8 => {
                if chars.len() != 1 {
                    return Err("UTF-8 character literal must contain one character".to_string());
                }
                if chars[0] > u8::MAX as u32 {
                    return Err("UTF-8 character literal value exceeds one byte".to_string());
                }
            }
            CharLiteralPrefix::Utf16 => {
                if chars.len() != 1 {
                    return Err("UTF-16 character literal must contain one character".to_string());
                }
                if chars[0] > 0xffff {
                    return Err("UTF-16 character literal value exceeds 16 bits".to_string());
                }
            }
            CharLiteralPrefix::Utf32 => {
                if chars.len() != 1 {
                    return Err("UTF-32 character literal must contain one character".to_string());
                }
            }
            CharLiteralPrefix::Wide | CharLiteralPrefix::None => {}
        }
        if prefix == CharLiteralPrefix::Wide && chars.len() > 1 {
            if let Some(decoded) = Self::decode_utf8_byte_chars(&chars) {
                let mut decoded_chars = decoded.chars();
                if decoded_chars.clone().count() == 1 {
                    if let Some(ch) = decoded_chars.next() {
                        value = ch as i64;
                    }
                }
            }
        }
        Ok(Token::CharLiteral(value))
    }

    fn read_string_literal(&mut self) -> Result<Token, String> {
        // Opening " already consumed
        let mut s = String::new();
        loop {
            match self.peek() {
                Some('"') => {
                    self.advance();
                    break;
                }
                Some('\n') | None => return Err("unterminated string literal".to_string()),
                _ => {
                    let c = self.unescape_char()?;
                    s.push(c);
                }
            }
        }
        Ok(Token::StringLiteral(s))
    }

    fn read_identifier_or_keyword(&mut self) -> Result<Token, String> {
        let mut word = String::new();
        while let Some((ch, len)) = self.peek_ident_continue() {
            word.push(ch);
            for _ in 0..len {
                self.advance();
            }
        }
        let token = match word.as_str() {
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
            "typedef" => Token::KWTypedef,
            "enum" => Token::KWEnum,
            "const" => Token::KWConst,
            "volatile" => Token::KWVolatile,
            "inline" => Token::KWInline,
            "__inline" => Token::KWInline,
            "__inline__" => Token::KWInline,
            "_Atomic" => Token::KWAtomic,
            "_Thread_local" | "thread_local" | "__thread" => Token::KWThreadLocal,
            "_Static_assert" | "static_assert" => Token::KWStaticAssert,
            "register" => Token::KWRegister,
            "auto" => Token::KWAuto,
            "_Bool" => Token::KWBool,
            "restrict" => Token::KWRestrict,
            "__restrict" => Token::KWRestrict,
            "__restrict__" => Token::KWRestrict,
            "short" => Token::KWShort,
            "_Noreturn" => Token::KWNoreturn,
            "_Generic" => Token::KWGeneric,
            "__auto_type" => Token::KWAutoType,
            "__extension__" | "_Nullable" | "_Nonnull" | "_Null_unspecified" | "__signed"
            | "__signed__" => Token::Skip, // gcc/clang extensions — skip
            "__asm__" | "__asm" | "asm" => {
                self.skip_whitespace_and_comments()?;
                loop {
                    let save = self.pos;
                    while self.peek().is_some_and(is_ident_continue) {
                        self.advance();
                    }
                    let word: String = self.chars[save..self.pos].iter().collect();
                    if !matches!(
                        word.as_str(),
                        "volatile" | "__volatile__" | "goto" | "inline"
                    ) {
                        self.pos = save;
                        break;
                    }
                    self.skip_whitespace_and_comments()?;
                }
                if self.peek() == Some('(') {
                    let mut depth = 0;
                    let asm_start = self.pos + 1;
                    loop {
                        match self.advance() {
                            Some('(') => depth += 1,
                            Some(')') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            None => break,
                            _ => {}
                        }
                    }
                    let asm_end = self.pos.saturating_sub(1);
                    let text: String = self.chars[asm_start..asm_end].iter().collect();
                    if let Some(tokens) = Self::asm_x87_math_tokens(&text) {
                        let mut tokens = VecDeque::from(tokens);
                        let first = tokens.pop_front().unwrap_or(Token::Skip);
                        self.pending_tokens.extend(tokens);
                        return Ok(first);
                    }
                    if let Some(tokens) = Self::asm_tied_zero_assignment_tokens(&text) {
                        let mut tokens = VecDeque::from(tokens);
                        let first = tokens.pop_front().unwrap_or(Token::Skip);
                        self.pending_tokens.extend(tokens);
                        return Ok(first);
                    }
                    if let Some(tokens) = Self::asm_simple_operand_side_effect_tokens(&text) {
                        let mut tokens = VecDeque::from(tokens);
                        let first = tokens.pop_front().unwrap_or(Token::Skip);
                        self.pending_tokens.extend(tokens);
                        return Ok(first);
                    }
                }
                Token::Skip
            }
            "__attribute__" | "__attribute" | "__declspec" => {
                // Skip common attribute annotations entirely — consume matching parens.
                self.skip_whitespace_and_comments()?;
                let attr_start = self.pos;
                if self.peek() == Some('(') {
                    let mut depth = 0;
                    loop {
                        match self.advance() {
                            Some('(') => depth += 1,
                            Some(')') => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            None => break,
                            _ => {}
                        }
                    }
                }
                let text: String = self.chars[attr_start..self.pos].iter().collect();
                if let Some(alias) = Self::parse_alias_attribute(&text) {
                    return Ok(Token::AttributeAlias(alias));
                }
                if let Some(mode) = Self::parse_mode_attribute(&text) {
                    return Ok(Token::AttributeMode(mode));
                }
                let vector_size = Self::parse_vector_size_attribute(&text);
                let deprecated = Self::parse_deprecated_attribute(&text);
                let alignment = Self::parse_aligned_attribute(&text);
                let noreturn = Self::contains_noreturn_attribute(&text);
                let no_instrument_function = Self::contains_no_instrument_function_attribute(&text);
                let packed = Self::contains_packed_attribute(&text);
                let transparent_union = Self::contains_transparent_union_attribute(&text);
                let reverse_scalar_storage_order =
                    Self::contains_reverse_scalar_storage_order_attribute(&text);
                if let Some(vector_size) = vector_size.clone() {
                    if alignment.is_some() || packed || noreturn || no_instrument_function {
                        self.pending_tokens
                            .push_back(Token::AttributeVectorSize(vector_size));
                    } else {
                        return Ok(Token::AttributeVectorSize(vector_size));
                    }
                }
                if let Some(alignment) = alignment {
                    return Ok(if packed && noreturn {
                        Token::AttributePackedAlignedNoreturn(alignment)
                    } else if packed {
                        Token::AttributePackedAligned(alignment)
                    } else if noreturn {
                        Token::AttributeAlignedNoreturn(alignment)
                    } else {
                        Token::AttributeAligned(alignment)
                    });
                }
                if packed {
                    return Ok(Token::AttributePacked);
                }
                if transparent_union {
                    return Ok(Token::AttributeTransparentUnion);
                }
                if reverse_scalar_storage_order {
                    return Ok(Token::AttributeScalarStorageOrderReverse);
                }
                if let Some(message) = deprecated {
                    return Ok(Token::AttributeDeprecated(message));
                }
                if noreturn {
                    return Ok(Token::AttributeNoreturn);
                }
                if no_instrument_function {
                    return Ok(Token::AttributeNoInstrumentFunction);
                }
                // Return dummy token that lex_all will filter out
                Token::Skip // harmless no-op token
            }
            "char" => Token::KWChar,
            "sizeof" => Token::KWSizeOf,
            "typeof" | "__typeof" | "__typeof__" | "typeof_unqual" | "__typeof_unqual"
            | "__typeof_unqual__" => Token::KWTypeOf,
            "_Alignof" | "alignof" | "__alignof" | "__alignof__" => Token::KWAlignOf,
            "_Alignas" | "alignas" => Token::KWAlignAs,
            "struct" => Token::KWStruct,
            "union" => Token::KWUnion,
            _ => Token::Identifier(word),
        };
        Ok(token)
    }

    fn asm_x87_math_tokens(text: &str) -> Option<Vec<Token>> {
        let mut parts = text.split(':');
        let template = parts.next()?.trim();
        let outputs = parts.next()?.trim();
        let inputs = parts.next()?.trim();
        let output_name = Self::first_parenthesized_identifier(outputs)?;
        let mut operands = Self::parenthesized_identifier_texts(inputs);
        let (builtin, args) = match template {
            "\"fsqrt\"" => {
                let input = operands.next()?.to_string();
                if operands.next().is_some() {
                    return None;
                }
                ("__builtin_sqrtl", vec![input])
            }
            "\"fpatan\\n\\t\"" | "\"fpatan\"" => {
                let x = operands.next()?.to_string();
                let y = operands.next()?.to_string();
                if operands.next().is_some() {
                    return None;
                }
                ("__builtin_atan2l", vec![y, x])
            }
            _ => return None,
        };
        let mut tokens = vec![
            Token::Identifier(output_name),
            Token::Assign,
            Token::Identifier(builtin.to_string()),
            Token::OpenParen,
        ];
        for (index, arg) in args.into_iter().enumerate() {
            if index > 0 {
                tokens.push(Token::Comma);
            }
            tokens.push(Self::simple_asm_value_token(arg.trim())?);
        }
        tokens.push(Token::CloseParen);
        Some(tokens)
    }

    fn asm_tied_zero_assignment_tokens(text: &str) -> Option<Vec<Token>> {
        let mut parts = text.split(':');
        let template = parts.next()?.trim();
        if !matches!(template, "\"\"" | "__extension__ \"\"") {
            return None;
        }
        let outputs = parts.next()?.trim();
        let inputs = parts.next()?.trim();
        let output_name = Self::first_parenthesized_identifier(outputs)?;
        if !inputs.contains("\"0\"") && !inputs.contains("'0'") {
            return None;
        }
        let value_text = Self::first_parenthesized_text(inputs)?;
        let value = value_text.trim();
        let value_tokens = Self::simple_asm_value_tokens(value)?;
        let mut tokens = vec![Token::Identifier(output_name), Token::Assign];
        tokens.extend(value_tokens);
        Some(tokens)
    }

    fn simple_asm_value_tokens(value: &str) -> Option<Vec<Token>> {
        if let Some(name) = value.trim().strip_prefix('&') {
            return Some(vec![
                Token::Ampersand,
                Self::simple_asm_value_token(name.trim())?,
            ]);
        }
        Some(vec![Self::simple_asm_value_token(value)?])
    }

    fn simple_asm_value_token(value: &str) -> Option<Token> {
        let integer = value
            .trim_end_matches(['l', 'L', 'u', 'U'])
            .trim()
            .parse::<i64>();
        if let Ok(value) = integer {
            return Some(Token::IntLiteral(value));
        }
        let mut chars = value.chars();
        let first = chars.next()?;
        if !(first.is_ascii_alphabetic() || first == '_') {
            return None;
        }
        if chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            Some(Token::Identifier(value.to_string()))
        } else {
            None
        }
    }

    fn asm_simple_operand_side_effect_tokens(text: &str) -> Option<Vec<Token>> {
        let mut parts = text.split(':');
        let template = parts.next()?.trim();
        if !matches!(template, "\"\"" | "__extension__ \"\"") {
            return None;
        }
        let outputs = parts.next()?.trim();
        let output = Self::last_parenthesized_text(outputs)?.trim();
        let call = output.strip_prefix('*').unwrap_or(output).trim();
        let name = call.strip_suffix("()")?.trim();
        let mut chars = name.chars();
        let first = chars.next()?;
        if !(first.is_ascii_alphabetic() || first == '_') {
            return None;
        }
        if !chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return None;
        }
        Some(vec![
            Token::Identifier(name.to_string()),
            Token::OpenParen,
            Token::CloseParen,
        ])
    }

    fn first_parenthesized_identifier(text: &str) -> Option<String> {
        let inner = Self::first_parenthesized_text(text)?.trim().to_string();
        let mut chars = inner.chars();
        let first = chars.next()?;
        if !(first.is_ascii_alphabetic() || first == '_') {
            return None;
        }
        if chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            Some(inner)
        } else {
            None
        }
    }

    fn first_parenthesized_text(text: &str) -> Option<&str> {
        let start = text.find('(')? + 1;
        let end = text[start..].find(')')? + start;
        Some(&text[start..end])
    }

    fn last_parenthesized_text(text: &str) -> Option<&str> {
        let end = text.rfind(')')?;
        let mut depth = 0usize;
        for (idx, ch) in text[..=end].char_indices().rev() {
            match ch {
                ')' => depth += 1,
                '(' => {
                    depth = depth.checked_sub(1)?;
                    if depth == 0 {
                        return Some(&text[idx + 1..end]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn parenthesized_identifier_texts(text: &str) -> impl Iterator<Item = &str> {
        text.match_indices('(').filter_map(|(start, _)| {
            let inner_start = start + 1;
            let end = text[inner_start..].find(')')? + inner_start;
            let inner = text[inner_start..end].trim();
            let mut chars = inner.chars();
            let first = chars.next()?;
            if !(first.is_ascii_alphabetic() || first == '_') {
                return None;
            }
            chars
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                .then_some(inner)
        })
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

    fn location_for_offset(&self, offset: usize) -> SourceLocation {
        let line_index = self.line_starts.partition_point(|start| *start <= offset);
        let line_index = line_index.saturating_sub(1);
        let line = line_index + 1;
        let column = offset.saturating_sub(self.line_starts[line_index]) + 1;
        if let Some(mapped) = self
            .line_map
            .as_ref()
            .and_then(|line_map| line_map.get(line.saturating_sub(1)))
        {
            return SourceLocation {
                file: mapped.file.clone(),
                line: mapped.line,
                column,
            };
        }
        SourceLocation {
            file: None,
            line,
            column,
        }
    }

    pub(crate) fn span_for_offsets(&self, start_offset: usize, end_offset: usize) -> SourceSpan {
        SourceSpan {
            start: self.location_for_offset(start_offset),
            end: self.location_for_offset(end_offset),
            start_offset,
            end_offset,
        }
    }

    pub fn lex_all_spanned(&mut self) -> Result<Vec<SpannedToken>, String> {
        let mut tokens = Vec::new();

        loop {
            self.skip_whitespace_and_comments()?;
            let start = self.pos;
            if let Some(tok) = self.pending_tokens.pop_front() {
                tokens.push(SpannedToken {
                    token: tok,
                    span: self.span_for_offsets(start, start),
                });
                continue;
            }
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
                '[' if self.peek() == Some('[') => {
                    self.advance();
                    let mut depth = 1usize;
                    let attr_start = self.pos;
                    while let Some(ch) = self.advance() {
                        if ch == '[' && self.peek() == Some('[') {
                            self.advance();
                            depth += 1;
                        } else if ch == ']' && self.peek() == Some(']') {
                            self.advance();
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                    }
                    let text: String = self.chars[attr_start..self.pos].iter().collect();
                    if Self::contains_noreturn_attribute(&text) {
                        Token::AttributeNoreturn
                    } else {
                        Token::Skip
                    }
                }
                '[' => Token::OpenBracket,
                ']' => Token::CloseBracket,
                '~' => Token::Tilde,
                '?' => Token::Question,
                ':' if self.peek() == Some('>') => {
                    self.advance();
                    Token::CloseBracket
                }
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
                    } else if self.peek() == Some('>') {
                        self.advance();
                        Token::Arrow
                    } else if self.peek() == Some('=') {
                        self.advance();
                        Token::MinusAssign
                    } else {
                        Token::Minus
                    }
                }
                '*' => self.two_char('=', Token::StarAssign, Token::Star),
                '/' => self.two_char('=', Token::SlashAssign, Token::Slash),
                '%' => {
                    if self.peek() == Some('>') {
                        self.advance();
                        Token::CloseBrace
                    } else {
                        self.two_char('=', Token::PercentAssign, Token::Percent)
                    }
                }

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
                    if self.peek() == Some('%') {
                        self.advance();
                        Token::OpenBrace
                    } else if self.peek() == Some(':') {
                        self.advance();
                        Token::OpenBracket
                    } else if self.peek() == Some('<') {
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
                '#' => {
                    while let Some(next) = self.peek() {
                        if next == '\n' {
                            break;
                        }
                        self.advance();
                    }
                    Token::Skip
                }

                '\'' => self.read_char_constant(CharLiteralPrefix::None)?,
                '"' => self.read_string_literal()?,
                'u' if self.peek() == Some('8') && self.peek_ahead(1) == Some('\'') => {
                    self.advance();
                    self.advance();
                    self.read_char_constant(CharLiteralPrefix::Utf8)?
                }
                'u' if self.peek() == Some('8') && self.peek_ahead(1) == Some('"') => {
                    self.advance();
                    self.advance();
                    self.read_string_literal()?
                }
                'u' | 'U' if self.peek() == Some('\'') => {
                    self.advance();
                    let prefix = if c == 'u' {
                        CharLiteralPrefix::Utf16
                    } else {
                        CharLiteralPrefix::Utf32
                    };
                    self.read_char_constant(prefix)?
                }
                'u' | 'U' if self.peek() == Some('"') => {
                    self.advance();
                    match self.read_string_literal()? {
                        Token::StringLiteral(s) if c == 'u' => Token::Utf16StringLiteral(s),
                        Token::StringLiteral(s) => Token::Utf32StringLiteral(s),
                        tok => tok,
                    }
                }
                'L' if self.peek() == Some('\'') => {
                    self.advance();
                    self.read_char_constant(CharLiteralPrefix::Wide)?
                }
                'L' if self.peek() == Some('"') => {
                    self.advance();
                    match self.read_string_literal()? {
                        Token::StringLiteral(s) => {
                            let chars: Vec<u32> = s.chars().map(|ch| ch as u32).collect();
                            let s = Self::decode_utf8_byte_chars(&chars).unwrap_or(s);
                            Token::WideStringLiteral(s)
                        }
                        tok => tok,
                    }
                }

                // Float literal starting with '.' (e.g., .5)
                '.' if self.peek().is_some_and(|c| c.is_ascii_digit()) => {
                    self.pos -= 1; // unget the '.'
                    self.read_number()?
                }
                '.' => {
                    if self.peek() == Some('.') {
                        self.advance(); // consume second .
                        if self.peek() == Some('.') {
                            self.advance(); // consume third .
                            Token::Ellipsis
                        } else {
                            return Err("expected '...' (three dots)".to_string());
                        }
                    } else {
                        Token::Dot
                    }
                }
                _ if c.is_ascii_digit() => {
                    self.pos -= 1; // unget
                    self.read_number()?
                }
                _ if is_ident_start(c) => {
                    self.pos -= 1; // unget
                    self.read_identifier_or_keyword()?
                }
                _ if c == '\\'
                    && self
                        .peek_ucn_at(self.pos - 1)
                        .is_some_and(|(ch, _)| is_ident_start(ch)) =>
                {
                    self.pos -= 1; // unget
                    self.read_identifier_or_keyword()?
                }
                _ if c == '\\' && self.starts_ucn_at(self.pos - 1) => {
                    let reason = self
                        .peek_ucn_at_result(self.pos - 1)
                        .expect_err("invalid UCN branch must report an error");
                    return Err(format!("{reason} at position {}", self.pos - 1));
                }

                _ => {
                    return Err(format!(
                        "unexpected character '{}' at position {}",
                        c,
                        self.pos - 1
                    ));
                }
            };

            // Filter out ignored annotations and extensions.
            if tok != Token::Skip {
                tokens.push(SpannedToken {
                    token: tok,
                    span: self.span_for_offsets(start, self.pos),
                });
            }
        }

        Ok(tokens)
    }

    #[allow(dead_code)]
    pub fn lex_all(&mut self) -> Result<Vec<Token>, String> {
        Ok(self
            .lex_all_spanned()?
            .into_iter()
            .map(|spanned| spanned.token)
            .collect())
    }
}

#[allow(dead_code)]
pub fn lex(input: &str) -> Result<Vec<Token>, String> {
    let mut lexer = Lexer::new(input);
    lexer.lex_all()
}

#[allow(dead_code)]
pub fn lex_spanned(input: &str) -> Result<Vec<SpannedToken>, String> {
    let mut lexer = Lexer::new(input);
    lexer.lex_all_spanned()
}

pub fn lex_spanned_with_line_map(
    input: &str,
    line_map: Vec<SourceLineMapping>,
) -> Result<Vec<SpannedToken>, String> {
    let mut lexer = Lexer::with_line_map(input, line_map);
    lexer.lex_all_spanned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn require_err<T>(result: Result<T, String>, context: &str) -> Result<String, String> {
        match result {
            Ok(_) => Err(format!("{context} unexpectedly succeeded")),
            Err(err) => Ok(err),
        }
    }

    #[test]
    fn reports_unterminated_block_comment() -> Result<(), String> {
        let err = require_err(lex("int main(void) { /* nope "), "lexing should fail")?;
        assert!(err.contains("unterminated block comment"));
        Ok(())
    }

    #[test]
    fn reports_invalid_number_suffix() -> Result<(), String> {
        let err = require_err(lex("int x = 123abc;"), "lexing should fail")?;
        assert!(err.contains("invalid number literal"));

        let err = require_err(lex("int x = 123α;"), "lexing should fail")?;
        assert!(err.contains("invalid number literal"));

        let err = require_err(lex("int x = 123\\U000003b1;"), "lexing should fail")?;
        assert!(err.contains("invalid number literal"));

        let err = require_err(lex("double x = .5Lfoo;"), "lexing should fail")?;
        assert!(err.contains("invalid float literal suffix"));
        Ok(())
    }

    #[test]
    fn reports_invalid_universal_character_names() -> Result<(), String> {
        let err = require_err(lex("int \\u12xz = 0;"), "lexing should fail")?;
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("malformed hex digits"));

        let err = require_err(lex("int \\U00110000 = 0;"), "lexing should fail")?;
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("out-of-range universal character"));

        let err = require_err(lex("int \\u0030 = 0;"), "lexing should fail")?;
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("basic character universal character"));

        let err = require_err(lex("int \\u0041 = 0;"), "lexing should fail")?;
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("basic character universal character"));
        Ok(())
    }

    #[test]
    fn lexes_unicode_identifier_letters() -> Result<(), String> {
        let tokens = lex("int α = 1; int β2 = α; int \\U000003b3 = \\u03b1;")?;
        assert!(tokens.contains(&Token::Identifier("α".to_string())));
        assert!(tokens.contains(&Token::Identifier("β2".to_string())));
        assert!(tokens.contains(&Token::Identifier("γ".to_string())));
        Ok(())
    }

    #[test]
    fn lexes_universal_character_escapes_in_literals() -> Result<(), String> {
        let tokens = lex("char *s = \"\\u03b1\\U000003b2\"; int c = '\\u03b3';")?;
        assert!(tokens.contains(&Token::StringLiteral("αβ".to_string())));
        assert!(tokens.contains(&Token::CharLiteral('γ' as i64)));

        let err = require_err(lex("char *s = \"\\u12xz\";"), "lexing should fail")?;
        assert!(err.contains("invalid universal character escape"));
        assert!(err.contains("malformed hex digits"));

        let err = require_err(lex("char *s = \"\\u0041\";"), "lexing should fail")?;
        assert!(err.contains("invalid universal character escape"));
        assert!(err.contains("basic character universal character"));
        Ok(())
    }

    #[test]
    fn lexes_utf_string_literal_prefixes() -> Result<(), String> {
        let tokens = lex("unsigned short *a = u\"hi\"; unsigned int *b = U\"π\";")?;
        assert!(tokens.contains(&Token::Utf16StringLiteral("hi".to_string())));
        assert!(tokens.contains(&Token::Utf32StringLiteral("π".to_string())));
        Ok(())
    }

    #[test]
    fn lexes_prefixed_character_literals() -> Result<(), String> {
        let tokens =
            lex("char *s = u8\"hi\"; int a = u'a'; int b = U'π'; int c = u8'x'; int d = L'a'; int *w = L\"hi\";")?;
        assert!(tokens.contains(&Token::StringLiteral("hi".to_string())));
        assert!(tokens.contains(&Token::CharLiteral('a' as i64)));
        assert!(tokens.contains(&Token::CharLiteral('π' as i64)));
        assert!(tokens.contains(&Token::CharLiteral('x' as i64)));
        assert!(tokens.contains(&Token::WideStringLiteral("hi".to_string())));

        let err = require_err(lex("int c = u'ab';"), "lexing should fail")?;
        assert!(err.contains("UTF-16 character literal must contain one character"));

        let err = require_err(lex("int c = u8'π';"), "lexing should fail")?;
        assert!(err.contains("UTF-8 character literal value exceeds one byte"));
        Ok(())
    }

    #[test]
    fn lexes_hexadecimal_octal_and_binary_integer_literals() -> Result<(), String> {
        let tokens = lex("int x = 0x2aU; int y = 052L; int z = 0b101010;")?;
        assert!(tokens.contains(&Token::UIntLiteral(42)));
        assert!(tokens.contains(&Token::LongLiteral(42)));
        assert!(tokens.contains(&Token::IntLiteral(42)));
        Ok(())
    }

    #[test]
    fn lexes_long_long_integer_suffixes_as_long() -> Result<(), String> {
        let tokens = lex("long x = 42LL; unsigned long y = 42ULL; long z = 42llu;")?;
        assert!(tokens.contains(&Token::LongLiteral(42)));
        assert_eq!(
            tokens
                .iter()
                .filter(|tok| matches!(tok, Token::ULongLiteral(42)))
                .count(),
            2
        );
        Ok(())
    }

    #[test]
    fn lexes_large_unsuffixed_hex_literals_as_unsigned_long() -> Result<(), String> {
        let tokens = lex("unsigned long x = 0xffffffff00000000;")?;
        assert!(tokens.contains(&Token::ULongLiteral(0xffffffff00000000u64 as i64)));
        Ok(())
    }

    #[test]
    fn lexes_leading_zero_float_literals_as_decimal_floats() -> Result<(), String> {
        let tokens = lex("double x = 0.0; double y = 09.5; long double z = 7.125L;")?;
        assert!(tokens.contains(&Token::DoubleLiteral(0.0)));
        assert!(tokens.contains(&Token::DoubleLiteral(9.5)));
        assert!(tokens.contains(&Token::LongDoubleLiteral(7.125)));
        Ok(())
    }

    #[test]
    fn lexes_hexadecimal_float_literals() -> Result<(), String> {
        let tokens = lex("double x = 0x1p2; double y = 0x1.8p+1F; double z = 0x.8p-1L;")?;
        assert!(tokens.contains(&Token::DoubleLiteral(4.0)));
        assert!(tokens.contains(&Token::DoubleLiteral(3.0)));
        assert!(tokens.contains(&Token::LongDoubleLiteral(0.25)));
        Ok(())
    }

    #[test]
    fn reports_invalid_hexadecimal_float_literals() -> Result<(), String> {
        let err = require_err(lex("double x = 0x1.;"), "lexing should fail")?;
        assert!(err.contains("missing binary exponent"));

        let err = require_err(lex("double x = 0x1p;"), "lexing should fail")?;
        assert!(err.contains("missing exponent digits"));
        Ok(())
    }

    #[test]
    fn lexes_hex_and_octal_character_escapes() -> Result<(), String> {
        let tokens = lex("int x = '\\x41'; int y = '\\101'; int z = '\\377';")?;
        assert!(tokens.contains(&Token::CharLiteral(65)));
        assert!(tokens.contains(&Token::CharLiteral(255)));
        Ok(())
    }

    #[test]
    fn lexes_wide_character_literals_without_panicking() -> Result<(), String> {
        let tokens = lex("int x = L'a'; int y = L'π';")?;
        assert!(tokens.contains(&Token::CharLiteral('a' as i64)));
        assert!(tokens.contains(&Token::CharLiteral('π' as i64)));
        Ok(())
    }

    #[test]
    fn reports_missing_hex_escape_digits() -> Result<(), String> {
        let err = require_err(lex("char *s = \"\\x\";"), "lexing should fail")?;
        assert!(err.contains("expected hexadecimal digits"));
        Ok(())
    }

    #[test]
    fn reports_invalid_octal_integer_literal() -> Result<(), String> {
        let err = require_err(lex("int x = 09;"), "lexing should fail")?;
        assert!(err.contains("invalid octal integer literal"));
        Ok(())
    }

    #[test]
    fn reports_invalid_binary_integer_literal() -> Result<(), String> {
        let err = require_err(lex("int x = 0b102;"), "lexing should fail")?;
        assert!(err.contains("invalid binary integer literal"));
        Ok(())
    }

    #[test]
    fn reports_unterminated_string_literal() -> Result<(), String> {
        let err = require_err(lex("char *s = \"unterminated;"), "lexing should fail")?;
        assert!(err.contains("unterminated string literal"));
        Ok(())
    }
}
