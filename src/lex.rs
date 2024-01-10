use crate::types::*;
use once_cell::sync::Lazy;
use regex::Regex;

fn id_to_tok(value: &str) -> Token {
    match value {
        "int" => Token::KWInt,
        "return" => Token::KWReturn,
        "void" => Token::KWVoid,
        other => Token::Identifier(String::from(other)),
    }
}

static ID_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Za-z_][A-Za-z0-9_]*\b").unwrap());
static CONST_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"[0-9]+\b").unwrap());

pub fn lex(input: &str) -> Vec<Token> {
    fn lex_helper(chars: &mut std::str::Chars) -> Vec<Token> {
        let mut tokens = Vec::new();
        while let Some(c) = chars.next() {
            match c {
                '{' => tokens.push(Token::OpenBrace),
                '}' => tokens.push(Token::CloseBrace),
                '(' => tokens.push(Token::OpenParen),
                ')' => tokens.push(Token::CloseParen),
                ';' => tokens.push(Token::Semicolon),
                _ if c.is_whitespace() => continue,
                _ if c.is_ascii_digit() => {
                    let rest = format!("{}{}", c, chars.collect::<String>());
                    if let Some(const_match) = CONST_REGEX.find(&rest) {
                        let const_str = const_match.as_str();
                        let parsed_const = const_str.parse::<i32>().unwrap();
                        tokens.push(Token::Constant(parsed_const));
                        let remaining = &rest[const_match.end()..];
                        let mut remaining_chars = remaining.chars();
                        tokens.append(&mut lex_helper(&mut remaining_chars));
                    } else {
                        panic!(
                            "Lexer failure: input starts with a digit but isn't a constant: {}",
                            rest
                        );
                    }
                    break;
                }
                _ => {
                    let rest = format!("{}{}", c, chars.collect::<String>());
                    if let Some(id_match) = ID_REGEX.find(&rest) {
                        let id_str = id_match.as_str();
                        let token = id_to_tok(id_str);
                        tokens.push(token);
                        let remaining = &rest[id_match.end()..];
                        let mut remaining_chars = remaining.chars();
                        tokens.append(&mut lex_helper(&mut remaining_chars));
                    } else {
                        panic!("Lexer failure: input doesn't match id_regexp: {}", rest);
                    }
                    break;
                }
            }
        }
        tokens
    }

    let trimmed_input = input.trim();
    let mut chars = trimmed_input.chars();
    lex_helper(&mut chars)
}
