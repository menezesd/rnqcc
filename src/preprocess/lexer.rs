use super::token::{PpLocation, PpSpan, PpToken, PpTokenKind};
use crate::types::{universal_character_name_error, validate_universal_character_value};

pub fn lex(input: &str) -> Result<Vec<PpToken>, String> {
    Lexer::new(&splice_continued_lines(&replace_trigraphs(input))).lex_all()
}

pub fn replace_trigraphs(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars.get(index) == Some(&'?') && chars.get(index + 1) == Some(&'?') {
            let replacement = match chars.get(index + 2).copied() {
                Some('=') => Some('#'),
                Some('/') => Some('\\'),
                Some('\'') => Some('^'),
                Some('(') => Some('['),
                Some(')') => Some(']'),
                Some('!') => Some('|'),
                Some('<') => Some('{'),
                Some('>') => Some('}'),
                Some('-') => Some('~'),
                _ => None,
            };
            if let Some(ch) = replacement {
                out.push(ch);
                index += 3;
                continue;
            }
        }
        out.push(chars[index]);
        index += 1;
    }
    out
}

fn splice_continued_lines(input: &str) -> String {
    let mut out = String::new();
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some('\n') => {
                    chars.next();
                    continue;
                }
                Some('\r') => {
                    chars.next();
                    if chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push(ch);
    }
    out
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: usize,
    column: usize,
    offset: usize,
}

impl Lexer {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
            line: 1,
            column: 1,
            offset: 0,
        }
    }

    fn lex_all(mut self) -> Result<Vec<PpToken>, String> {
        let mut tokens = Vec::new();
        while self.peek().is_some() {
            let start = self.location();
            let kind = match self.peek() {
                Some('\n') => {
                    let mut text = String::new();
                    text.push(self.advance_required("newline")?);
                    PpTokenKind::Newline(text)
                }
                Some('\r') => {
                    let mut text = String::new();
                    text.push(self.advance_required("newline")?);
                    if self.peek() == Some('\n') {
                        text.push(self.advance_required("newline")?);
                    }
                    PpTokenKind::Newline(text)
                }
                Some(ch) if ch == ' ' || ch == '\t' || ch == '\x0c' || ch == '\x0b' => {
                    self.read_whitespace()
                }
                Some('/') if self.peek_ahead(1) == Some('/') => self.read_line_comment()?,
                Some('/') if self.peek_ahead(1) == Some('*') => self.read_block_comment()?,
                Some(ch)
                    if starts_string_or_char_literal(
                        ch,
                        self.peek_ahead(1),
                        self.peek_ahead(2),
                    ) =>
                {
                    self.read_string_or_char()?
                }
                Some(ch) if is_ident_start(ch) => self.read_ident(),
                Some('\\') if self.peek_ucn().is_some_and(|(ch, _)| is_ident_start(ch)) => {
                    self.read_ident()
                }
                Some('\\') if matches!(self.peek_ahead(1), Some('u' | 'U')) => {
                    let reason = self
                        .peek_ucn_result()
                        .expect_err("invalid UCN branch must report an error");
                    return Err(format!(
                        "{} at line {}, column {}",
                        reason, start.line, start.column
                    ));
                }
                Some(ch)
                    if ch.is_ascii_digit()
                        || ch == '.' && self.peek_ahead(1).is_some_and(|c| c.is_ascii_digit()) =>
                {
                    self.read_number()
                }
                Some(_) => self.read_punct()?,
                None => break,
            };
            tokens.push(PpToken {
                kind,
                span: PpSpan {
                    start,
                    end: self.location(),
                },
            });
        }
        Ok(tokens)
    }

    fn location(&self) -> PpLocation {
        PpLocation {
            line: self.line,
            column: self.column,
            offset: self.offset,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_ahead(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn peek_ucn(&self) -> Option<(char, usize)> {
        self.peek_ucn_result().ok().flatten()
    }

    fn peek_ucn_result(&self) -> Result<Option<(char, usize)>, String> {
        if self.peek() != Some('\\') {
            return Ok(None);
        }
        let (digits, len) = match self.peek_ahead(1) {
            Some('u') => (4, 6),
            Some('U') => (8, 10),
            Some(_) => return Ok(None),
            None => return Ok(None),
        };
        let mut value = 0u32;
        for offset in 0..digits {
            let Some(ch) = self.peek_ahead(2 + offset) else {
                return Err(universal_character_name_error("incomplete spelling"));
            };
            let Some(digit) = ch.to_digit(16) else {
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

    fn peek_ident_continue(&self) -> Option<(char, usize)> {
        if let Some(ch) = self.peek().filter(|ch| is_ident_continue(*ch)) {
            Some((ch, 1))
        } else {
            self.peek_ucn().filter(|(ch, _)| is_ident_continue(*ch))
        }
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.peek()?;
        self.pos += 1;
        self.offset += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(ch)
    }

    fn advance_required(&mut self, context: &str) -> Result<char, String> {
        self.advance().ok_or_else(|| {
            format!(
                "unexpected end of preprocessor input while reading {}",
                context
            )
        })
    }

    fn read_whitespace(&mut self) -> PpTokenKind {
        let mut text = String::new();
        while let Some(ch) = self.peek() {
            if ch == ' ' || ch == '\t' || ch == '\x0c' || ch == '\x0b' {
                text.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        PpTokenKind::Whitespace(text)
    }

    fn read_line_comment(&mut self) -> Result<PpTokenKind, String> {
        let mut text = String::new();
        text.push(self.advance_required("line comment")?);
        text.push(self.advance_required("line comment")?);
        while let Some(ch) = self.peek() {
            if ch == '\n' || ch == '\r' {
                break;
            }
            text.push(ch);
            self.advance();
        }
        Ok(PpTokenKind::Whitespace(text))
    }

    fn read_block_comment(&mut self) -> Result<PpTokenKind, String> {
        let start = self.location();
        let mut text = String::new();
        text.push(self.advance_required("block comment")?);
        text.push(self.advance_required("block comment")?);
        while let Some(ch) = self.advance() {
            text.push(ch);
            if ch == '*' && self.peek() == Some('/') {
                text.push(self.advance_required("block comment")?);
                return Ok(PpTokenKind::Whitespace(text));
            }
        }
        Err(format!(
            "unterminated block comment at line {}, column {}",
            start.line, start.column
        ))
    }

    fn read_string_or_char(&mut self) -> Result<PpTokenKind, String> {
        let mut text = String::new();
        if self.peek() == Some('u') && self.peek_ahead(1) == Some('8') {
            text.push(self.advance_required("literal prefix")?);
            text.push(self.advance_required("literal prefix")?);
        } else if matches!(self.peek(), Some('u' | 'U' | 'L')) {
            text.push(self.advance_required("literal prefix")?);
        }
        let quote = self
            .peek()
            .ok_or_else(|| "unexpected end of preprocessor input before literal".to_string())?;
        let kind = if quote == '"' {
            PpTokenKind::StringLit
        } else {
            PpTokenKind::CharLit
        };
        self.read_quoted_with_prefix(quote, text, kind)
    }

    fn read_quoted_with_prefix(
        &mut self,
        quote: char,
        mut text: String,
        kind: fn(String) -> PpTokenKind,
    ) -> Result<PpTokenKind, String> {
        text.push(self.advance_required("literal")?);
        let mut escaped = false;
        while let Some(ch) = self.advance() {
            text.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                return Ok(kind(text));
            } else if ch == '\n' || ch == '\r' {
                return Err("unterminated literal in preprocessor input".to_string());
            }
        }
        Err("unterminated literal in preprocessor input".to_string())
    }

    fn read_ident(&mut self) -> PpTokenKind {
        let mut text = String::new();
        while let Some((ch, len)) = self.peek_ident_continue() {
            text.push(ch);
            for _ in 0..len {
                self.advance();
            }
        }
        PpTokenKind::Ident(text)
    }

    fn read_number(&mut self) -> PpTokenKind {
        let mut text = String::new();
        while let Some(ch) = self.peek() {
            if ch.is_ascii_alphanumeric()
                || matches!(ch, '_' | '.')
                || matches!(ch, '+' | '-')
                    && matches!(text.chars().last(), Some('e' | 'E' | 'p' | 'P'))
            {
                text.push(ch);
                self.advance();
            } else {
                break;
            }
        }
        PpTokenKind::Number(text)
    }

    fn read_punct(&mut self) -> Result<PpTokenKind, String> {
        const PUNCTS: [&str; 54] = [
            "%:%:", "<<=", ">>=", "...", "##", "->", "++", "--", "<<", ">>", "<=", ">=", "==",
            "!=", "&&", "||", "*=", "/=", "%=", "+=", "-=", "&=", "^=", "|=", "%:", "<:", ":>",
            "<%", "%>", "[", "]", "(", ")", "{", "}", ".", "&", "*", "+", "-", "~", "!", "/", "%",
            "<", ">", "^", "|", "?", ":", ";", "=", ",", "#",
        ];
        let rest: String = self.chars[self.pos..].iter().collect();
        for punct in PUNCTS {
            if rest.starts_with(punct) {
                for _ in punct.chars() {
                    self.advance();
                }
                return Ok(PpTokenKind::Punct(punct.to_string()));
            }
        }
        Ok(PpTokenKind::Punct(
            self.advance_required("punctuator")?.to_string(),
        ))
    }
}

pub(crate) fn is_ident_start(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_start(ch)
}

pub(crate) fn is_ident_continue(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_continue(ch)
}

fn starts_string_or_char_literal(ch: char, next: Option<char>, after_next: Option<char>) -> bool {
    matches!(ch, '"' | '\'')
        || (matches!(ch, 'u' | 'U' | 'L') && matches!(next, Some('"' | '\'')))
        || (ch == 'u' && next == Some('8') && matches!(after_next, Some('"' | '\'')))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(input: &str) -> Result<Vec<PpTokenKind>, String> {
        Ok(lex(input)?.into_iter().map(|token| token.kind).collect())
    }

    #[test]
    fn lexes_identifiers_numbers_and_punctuators() -> Result<(), String> {
        let got = kinds("FOO 123 0x1p-3 ## ... <<= >= # %:%: <% %>")?;
        assert_eq!(
            got,
            vec![
                PpTokenKind::Ident("FOO".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Number("123".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Number("0x1p-3".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct("##".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct("...".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct("<<=".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct(">=".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct("#".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct("%:%:".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct("<%".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Punct("%>".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn lexes_unicode_identifier_letters() -> Result<(), String> {
        let got = kinds("α β2 _γ \\U000003b4 \\u03b5")?;
        assert_eq!(
            got,
            vec![
                PpTokenKind::Ident("α".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Ident("β2".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Ident("_γ".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Ident("δ".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Ident("ε".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn rejects_invalid_universal_character_names() -> Result<(), String> {
        let err = lex("\\u12xz").expect_err("lexing should fail");
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("malformed hex digits"));

        let err = lex("\\U00110000").expect_err("lexing should fail");
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("out-of-range universal character"));

        let err = lex("\\u0030").expect_err("lexing should fail");
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("basic character universal character"));

        let err = lex("\\u0041").expect_err("lexing should fail");
        assert!(err.contains("invalid universal character name"));
        assert!(err.contains("basic character universal character"));
        Ok(())
    }

    #[test]
    fn turns_comments_into_whitespace() -> Result<(), String> {
        let got = kinds("a/* hidden */b// line\nc")?;
        assert_eq!(
            got,
            vec![
                PpTokenKind::Ident("a".to_string()),
                PpTokenKind::Whitespace("/* hidden */".to_string()),
                PpTokenKind::Ident("b".to_string()),
                PpTokenKind::Whitespace("// line".to_string()),
                PpTokenKind::Newline("\n".to_string()),
                PpTokenKind::Ident("c".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn preserves_string_and_char_literals() -> Result<(), String> {
        let got = kinds("u8\"a\\\\n\" L'x' '\\''")?;
        assert_eq!(
            got,
            vec![
                PpTokenKind::StringLit("u8\"a\\\\n\"".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::CharLit("L'x'".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::CharLit("'\\''".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn removes_backslash_newline_splices() -> Result<(), String> {
        let got = kinds("A\\\nB")?;
        assert_eq!(got, vec![PpTokenKind::Ident("AB".to_string())]);
        Ok(())
    }

    #[test]
    fn replaces_hash_trigraph_before_tokenization() -> Result<(), String> {
        let got = kinds("??=define FOO 1")?;
        assert_eq!(
            got,
            vec![
                PpTokenKind::Punct("#".to_string()),
                PpTokenKind::Ident("define".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Ident("FOO".to_string()),
                PpTokenKind::Whitespace(" ".to_string()),
                PpTokenKind::Number("1".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn replaces_backslash_trigraph_before_newline_splicing() -> Result<(), String> {
        let got = kinds("A??/\nB")?;
        assert_eq!(got, vec![PpTokenKind::Ident("AB".to_string())]);
        Ok(())
    }

    #[test]
    fn replaces_bracket_and_brace_trigraph_punctuators() -> Result<(), String> {
        let got = kinds("a??(0??)??<b??>")?;
        assert_eq!(
            got,
            vec![
                PpTokenKind::Ident("a".to_string()),
                PpTokenKind::Punct("[".to_string()),
                PpTokenKind::Number("0".to_string()),
                PpTokenKind::Punct("]".to_string()),
                PpTokenKind::Punct("{".to_string()),
                PpTokenKind::Ident("b".to_string()),
                PpTokenKind::Punct("}".to_string()),
            ]
        );
        Ok(())
    }

    #[test]
    fn preserves_crlf_newline_spelling() -> Result<(), String> {
        let got = kinds("A\r\nB")?;
        assert_eq!(
            got,
            vec![
                PpTokenKind::Ident("A".to_string()),
                PpTokenKind::Newline("\r\n".to_string()),
                PpTokenKind::Ident("B".to_string()),
            ]
        );
        Ok(())
    }
}
