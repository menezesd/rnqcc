use super::macro_expand::MacroDef;
use super::token::{PpToken, PpTokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeaderName {
    Quoted(String),
    Angled(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncludeOperand {
    Literal(HeaderName),
    Tokens(Vec<PpToken>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineOperand {
    Literal {
        line: usize,
        filename: Option<String>,
    },
    Tokens(Vec<PpToken>),
    Malformed(LineOperandError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineOperandError {
    MissingLine,
    InvalidLine(String),
    InvalidFilename(Vec<PpToken>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Directive {
    Include {
        operand: IncludeOperand,
        include_next: bool,
    },
    Define {
        name: String,
        def: MacroDef,
    },
    Undef {
        name: String,
    },
    If {
        expr: Vec<PpToken>,
    },
    Ifdef {
        name: String,
        negated: bool,
    },
    Elif {
        expr: Vec<PpToken>,
    },
    Elifdef {
        name: String,
        negated: bool,
    },
    Else,
    Endif,
    Line {
        tokens: Vec<PpToken>,
    },
    Pragma {
        tokens: Vec<PpToken>,
    },
    Error {
        tokens: Vec<PpToken>,
    },
    Warning {
        tokens: Vec<PpToken>,
    },
    Ident,
    Empty,
    Unknown {
        name: String,
        tokens: Vec<PpToken>,
    },
}

pub fn directive_name(tokens: &[PpToken]) -> Option<&str> {
    let hash = skip_ws(tokens, 0);
    if !is_hash(tokens.get(hash)) {
        return None;
    }
    let name_index = skip_ws(tokens, hash + 1);
    ident_text(tokens.get(name_index))
}

pub fn parse_directive_tokens(tokens: &[PpToken]) -> Result<Option<Directive>, String> {
    let hash = skip_ws(tokens, 0);
    if !is_hash(tokens.get(hash)) {
        return Ok(None);
    }
    let name_index = skip_ws(tokens, hash + 1);
    let Some(name) = ident_text(tokens.get(name_index)) else {
        return Ok(Some(Directive::Empty));
    };
    let rest = trim_leading_ws(&tokens[name_index + 1..]);
    Ok(Some(match name {
        "include" => Directive::Include {
            operand: parse_include_operand(rest),
            include_next: false,
        },
        "include_next" => Directive::Include {
            operand: parse_include_operand(rest),
            include_next: true,
        },
        "define" => parse_define(rest)?,
        "undef" => Directive::Undef {
            name: required_single_ident(rest, "#undef")?.to_string(),
        },
        "if" => Directive::If {
            expr: trim_tokens(rest),
        },
        "ifdef" => Directive::Ifdef {
            name: required_single_ident(rest, "#ifdef")?.to_string(),
            negated: false,
        },
        "ifndef" => Directive::Ifdef {
            name: required_single_ident(rest, "#ifndef")?.to_string(),
            negated: true,
        },
        "elif" => Directive::Elif {
            expr: trim_tokens(rest),
        },
        "elifdef" => Directive::Elifdef {
            name: required_single_ident(rest, "#elifdef")?.to_string(),
            negated: false,
        },
        "elifndef" => Directive::Elifdef {
            name: required_single_ident(rest, "#elifndef")?.to_string(),
            negated: true,
        },
        "else" => {
            require_no_operands(rest, "#else")?;
            Directive::Else
        }
        "endif" => {
            require_no_operands(rest, "#endif")?;
            Directive::Endif
        }
        "line" => Directive::Line {
            tokens: trim_tokens(rest),
        },
        "pragma" => Directive::Pragma {
            tokens: trim_tokens(rest),
        },
        "error" => Directive::Error {
            tokens: trim_tokens(rest),
        },
        "warning" => Directive::Warning {
            tokens: trim_tokens(rest),
        },
        "ident" => Directive::Ident,
        other => Directive::Unknown {
            name: other.to_string(),
            tokens: trim_tokens(rest),
        },
    }))
}

fn parse_define(tokens: &[PpToken]) -> Result<Directive, String> {
    let name_index = skip_ws(tokens, 0);
    let name = required_ident(tokens, "#define")?.to_string();
    if is_function_like_define(tokens, name_index) {
        let close = find_matching_paren(tokens, name_index + 1)
            .ok_or_else(|| format!("missing ')' in function-like macro {}", name))?;
        let (params, variadic) = parse_macro_params(&tokens[name_index + 2..close])?;
        let replacement_start = skip_ws(tokens, close + 1);
        return Ok(Directive::Define {
            name,
            def: MacroDef::Function {
                params,
                variadic,
                body: trim_tokens(&tokens[replacement_start..]),
            },
        });
    }
    let replacement_start = skip_ws(tokens, name_index + 1);
    Ok(Directive::Define {
        name,
        def: MacroDef::Object(trim_tokens(&tokens[replacement_start..])),
    })
}

fn is_function_like_define(tokens: &[PpToken], name_index: usize) -> bool {
    let Some(name) = tokens.get(name_index) else {
        return false;
    };
    let Some(open) = tokens.get(name_index + 1) else {
        return false;
    };
    matches!(&open.kind, PpTokenKind::Punct(value) if value == "(")
        && name.span.end.offset == open.span.start.offset
}

fn find_matching_paren(tokens: &[PpToken], open_index: usize) -> Option<usize> {
    if !matches!(tokens.get(open_index).map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == "(")
    {
        return None;
    }
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index + 1) {
        if matches!(&token.kind, PpTokenKind::Punct(value) if value == "(") {
            depth += 1;
        } else if matches!(&token.kind, PpTokenKind::Punct(value) if value == ")") {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
        }
    }
    None
}

fn parse_macro_params(tokens: &[PpToken]) -> Result<(Vec<String>, bool), String> {
    let mut params = Vec::new();
    let mut variadic = false;
    let mut index = skip_ws(tokens, 0);
    if index >= tokens.len() {
        return Ok((params, variadic));
    }
    loop {
        index = skip_ws(tokens, index);
        if matches!(tokens.get(index).map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == "...")
        {
            reject_duplicate_macro_param(&params, "__VA_ARGS__")?;
            variadic = true;
            index += 1;
        } else if let Some(name) = ident_text(tokens.get(index)) {
            reject_reserved_macro_param(name)?;
            reject_duplicate_macro_param(&params, name)?;
            params.push(name.to_string());
            index += 1;
            let after_name = skip_ws(tokens, index);
            if matches!(tokens.get(after_name).map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == "...")
            {
                reject_duplicate_macro_param(&params, "__VA_ARGS__")?;
                variadic = true;
                index = after_name + 1;
            }
        } else {
            return Err("expected macro parameter name".to_string());
        }
        index = skip_ws(tokens, index);
        if index >= tokens.len() {
            break;
        }
        if matches!(tokens.get(index).map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == ",")
        {
            if variadic {
                return Err("variadic parameter must be last".to_string());
            }
            index += 1;
            continue;
        }
        return Err("expected ',' in macro parameter list".to_string());
    }
    Ok((params, variadic))
}

fn reject_reserved_macro_param(name: &str) -> Result<(), String> {
    if matches!(name, "__VA_ARGS__" | "__VA_OPT__") {
        return Err(format!("reserved macro parameter name {}", name));
    }
    Ok(())
}

fn reject_duplicate_macro_param(params: &[String], name: &str) -> Result<(), String> {
    if params.iter().any(|param| param == name) {
        return Err(format!("duplicate macro parameter name {}", name));
    }
    Ok(())
}

pub fn parse_include_operand(tokens: &[PpToken]) -> IncludeOperand {
    match literal_header(tokens) {
        Some(header) => IncludeOperand::Literal(header),
        None => IncludeOperand::Tokens(trim_tokens(tokens)),
    }
}

pub fn parse_line_operand(tokens: &[PpToken]) -> LineOperand {
    let tokens = trim_tokens(tokens);
    let Some(first) = tokens.first() else {
        return LineOperand::Malformed(LineOperandError::MissingLine);
    };
    let PpTokenKind::Number(line_text) = &first.kind else {
        return LineOperand::Tokens(tokens);
    };
    let Ok(line) = line_text.parse::<usize>() else {
        return LineOperand::Malformed(LineOperandError::InvalidLine(line_text.clone()));
    };
    let filename_index = skip_ws(&tokens, 1);
    if filename_index == tokens.len() {
        return LineOperand::Literal {
            line,
            filename: None,
        };
    }
    if matches!(
        tokens.get(filename_index).map(|token| &token.kind),
        Some(PpTokenKind::Ident(_))
    ) {
        return LineOperand::Tokens(tokens);
    }
    if let Some(PpToken {
        kind: PpTokenKind::StringLit(filename),
        ..
    }) = tokens.get(filename_index)
    {
        let trailing = skip_ws(&tokens, filename_index + 1);
        if trailing == tokens.len() {
            return LineOperand::Literal {
                line,
                filename: Some(quoted_header_name(filename)),
            };
        }
    }
    LineOperand::Malformed(LineOperandError::InvalidFilename(
        tokens[filename_index..].to_vec(),
    ))
}

fn literal_header(tokens: &[PpToken]) -> Option<HeaderName> {
    let tokens = trim_tokens(tokens);
    match tokens.first().map(|token| &token.kind) {
        Some(PpTokenKind::StringLit(text)) if tokens.len() == 1 => {
            Some(HeaderName::Quoted(quoted_header_name(text)))
        }
        Some(PpTokenKind::Punct(open)) if open == "<" => {
            let mut name = String::new();
            for (index, token) in tokens.iter().enumerate().skip(1) {
                if matches!(&token.kind, PpTokenKind::Punct(value) if value == ">") {
                    return (index + 1 == tokens.len()).then_some(HeaderName::Angled(name));
                }
                name.push_str(token.text());
            }
            None
        }
        _ => None,
    }
}

fn quoted_header_name(text: &str) -> String {
    text.strip_prefix('"')
        .and_then(|text| text.strip_suffix('"'))
        .unwrap_or(text)
        .to_string()
}

fn required_ident<'a>(tokens: &'a [PpToken], directive: &str) -> Result<&'a str, String> {
    let index = skip_ws(tokens, 0);
    ident_text(tokens.get(index)).ok_or_else(|| format!("expected identifier after {}", directive))
}

fn required_single_ident<'a>(tokens: &'a [PpToken], directive: &str) -> Result<&'a str, String> {
    let index = skip_ws(tokens, 0);
    let name = ident_text(tokens.get(index))
        .ok_or_else(|| format!("expected identifier after {}", directive))?;
    let trailing = skip_ws(tokens, index + 1);
    if trailing != tokens.len() {
        return Err(format!("unexpected tokens after {} identifier", directive));
    }
    Ok(name)
}

fn require_no_operands(tokens: &[PpToken], directive: &str) -> Result<(), String> {
    let trailing = skip_ws(tokens, 0);
    if trailing != tokens.len() {
        return Err(format!("unexpected tokens after {}", directive));
    }
    Ok(())
}

fn ident_text(token: Option<&PpToken>) -> Option<&str> {
    match token.map(|token| &token.kind) {
        Some(PpTokenKind::Ident(value)) => Some(value),
        _ => None,
    }
}

fn skip_ws(tokens: &[PpToken], mut index: usize) -> usize {
    while matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(PpTokenKind::Whitespace(_))
    ) {
        index += 1;
    }
    index
}

fn trim_leading_ws(tokens: &[PpToken]) -> &[PpToken] {
    &tokens[skip_ws(tokens, 0)..]
}

fn trim_tokens(tokens: &[PpToken]) -> Vec<PpToken> {
    let start = skip_ws(tokens, 0);
    let end = tokens
        .iter()
        .rposition(|token| !matches!(token.kind, PpTokenKind::Whitespace(_)))
        .map(|index| index + 1)
        .unwrap_or(start);
    tokens[start..end].to_vec()
}

fn is_hash(token: Option<&PpToken>) -> bool {
    matches!(
        token.map(|token| &token.kind),
        Some(PpTokenKind::Punct(value)) if value == "#" || value == "%:"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocess::lexer::lex;

    fn directive(input: &str) -> Result<Directive, String> {
        parse_directive_tokens(&lex(input)?)?.ok_or_else(|| "expected directive".to_string())
    }

    fn line_operand(input: &str) -> Result<LineOperand, String> {
        let Directive::Line { tokens } = directive(input)? else {
            return Err("expected line directive".to_string());
        };
        Ok(parse_line_operand(&tokens))
    }

    #[test]
    fn parses_include_literals() -> Result<(), String> {
        assert_eq!(
            directive("#include <stdio.h>")?,
            Directive::Include {
                operand: IncludeOperand::Literal(HeaderName::Angled("stdio.h".to_string())),
                include_next: false,
            }
        );
        assert_eq!(
            directive("#include \"local.h\"")?,
            Directive::Include {
                operand: IncludeOperand::Literal(HeaderName::Quoted("local.h".to_string())),
                include_next: false,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_quoted_include_literals_with_escapes_and_spaces() -> Result<(), String> {
        assert_eq!(
            directive(r#"#include "dir/a\\\" spaced header.h""#)?,
            Directive::Include {
                operand: IncludeOperand::Literal(HeaderName::Quoted(
                    r#"dir/a\\\" spaced header.h"#.to_string()
                )),
                include_next: false,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_angled_include_literals_with_inner_spaces() -> Result<(), String> {
        assert_eq!(
            directive("#include <my headers/local config.h>")?,
            Directive::Include {
                operand: IncludeOperand::Literal(HeaderName::Angled(
                    "my headers/local config.h".to_string()
                )),
                include_next: false,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_angled_include_literals_with_path_punctuation() -> Result<(), String> {
        assert_eq!(
            directive("#include <vendor-1/sys.bits/config+abi_v2.h>")?,
            Directive::Include {
                operand: IncludeOperand::Literal(HeaderName::Angled(
                    "vendor-1/sys.bits/config+abi_v2.h".to_string()
                )),
                include_next: false,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_macro_include_operands_as_tokens() -> Result<(), String> {
        let Directive::Include { operand, .. } = directive("#include HEADER")? else {
            return Err("expected include".to_string());
        };
        assert!(matches!(operand, IncludeOperand::Tokens(tokens) if !tokens.is_empty()));
        Ok(())
    }

    #[test]
    fn parses_function_macro_include_operands_as_tokens() -> Result<(), String> {
        let Directive::Include { operand, .. } = directive("#include HEADER_NAME(\"local.h\")")?
        else {
            return Err("expected include".to_string());
        };
        assert!(matches!(
            operand,
            IncludeOperand::Tokens(tokens)
                if matches!(tokens.as_slice(), [PpToken { kind: PpTokenKind::Ident(name), .. }, ..] if name == "HEADER_NAME")
        ));
        Ok(())
    }

    #[test]
    fn preserves_malformed_unterminated_headers_as_tokens() -> Result<(), String> {
        let Directive::Include { operand, .. } = directive("#include <unterminated/header.h")?
        else {
            return Err("expected include".to_string());
        };
        assert!(matches!(operand, IncludeOperand::Tokens(tokens) if !tokens.is_empty()));

        assert!(lex(r#"#include "unterminated/header.h"#).is_err());
        Ok(())
    }

    #[test]
    fn preserves_include_operands_with_extra_trailing_tokens_as_tokens() -> Result<(), String> {
        for input in [r#"#include "local.h" extra"#, "#include <stdio.h> extra"] {
            let Directive::Include { operand, .. } = directive(input)? else {
                return Err("expected include".to_string());
            };
            assert!(matches!(operand, IncludeOperand::Tokens(tokens) if !tokens.is_empty()));
        }
        Ok(())
    }

    #[test]
    fn parses_digraph_hash_directives() -> Result<(), String> {
        assert!(matches!(
            directive("%:define DIGRAPH_VALUE 11")?,
            Directive::Define { name, def: MacroDef::Object(replacement) }
                if name == "DIGRAPH_VALUE"
                    && matches!(replacement.as_slice(), [PpToken { kind: PpTokenKind::Number(value), .. }] if value == "11")
        ));
        assert_eq!(
            directive(" \t%: include <digraph.h>")?,
            Directive::Include {
                operand: IncludeOperand::Literal(HeaderName::Angled("digraph.h".to_string())),
                include_next: false,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_empty_directives() -> Result<(), String> {
        assert_eq!(directive("#")?, Directive::Empty);
        assert_eq!(directive(" \t%: \t")?, Directive::Empty);
        Ok(())
    }

    #[test]
    fn parses_define_with_space_after_hash() -> Result<(), String> {
        assert!(matches!(
            directive("# define SPACED 7")?,
            Directive::Define { name, def: MacroDef::Object(replacement) }
                if name == "SPACED"
                    && matches!(replacement.as_slice(), [PpToken { kind: PpTokenKind::Number(value), .. }] if value == "7")
        ));
        Ok(())
    }

    #[test]
    fn parses_define_and_conditionals() -> Result<(), String> {
        assert!(matches!(
            directive("#define A 42")?,
            Directive::Define { name, def: MacroDef::Object(replacement) } if name == "A" && !replacement.is_empty()
        ));
        assert_eq!(
            directive("#elifndef MISSING")?,
            Directive::Elifdef {
                name: "MISSING".to_string(),
                negated: true,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_line_operand_numeric_line() -> Result<(), String> {
        assert_eq!(
            line_operand("#line 123")?,
            LineOperand::Literal {
                line: 123,
                filename: None,
            }
        );
        Ok(())
    }

    #[test]
    fn parses_line_operand_optional_string_filename() -> Result<(), String> {
        assert_eq!(
            line_operand(r#"#line 456 "generated.c""#)?,
            LineOperand::Literal {
                line: 456,
                filename: Some("generated.c".to_string()),
            }
        );
        Ok(())
    }

    #[test]
    fn preserves_line_macro_operands_as_tokens_for_expansion() -> Result<(), String> {
        assert!(matches!(
            line_operand("#line LINE_NUMBER FILE_NAME")?,
            LineOperand::Tokens(tokens)
                if matches!(tokens.as_slice(), [PpToken { kind: PpTokenKind::Ident(line), .. }, ..] if line == "LINE_NUMBER")
        ));
        assert!(matches!(
            line_operand("#line 77 FILE_NAME")?,
            LineOperand::Tokens(tokens)
                if matches!(
                    tokens.as_slice(),
                    [
                        PpToken { kind: PpTokenKind::Number(line), .. },
                        PpToken { kind: PpTokenKind::Whitespace(_), .. },
                        PpToken { kind: PpTokenKind::Ident(filename), .. },
                    ] if line == "77" && filename == "FILE_NAME"
                )
        ));
        Ok(())
    }

    #[test]
    fn rejects_trailing_tokens_after_single_identifier_directives() -> Result<(), String> {
        for input in [
            "#undef NAME EXTRA",
            "#ifdef NAME EXTRA",
            "#ifndef NAME EXTRA",
            "#elifdef NAME EXTRA",
            "#elifndef NAME EXTRA",
        ] {
            assert!(
                directive(input).is_err(),
                "expected {input:?} to reject trailing tokens"
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_trailing_tokens_after_operandless_directives() -> Result<(), String> {
        for input in ["#else EXTRA", "#endif EXTRA"] {
            assert!(
                directive(input).is_err(),
                "expected {input:?} to reject trailing tokens"
            );
        }
        Ok(())
    }

    #[test]
    fn preserves_ident_directive_payload_as_ignored() -> Result<(), String> {
        assert_eq!(directive(r#"#ident "ignored""#)?, Directive::Ident);
        Ok(())
    }

    #[test]
    fn reports_malformed_line_operand_missing_line() -> Result<(), String> {
        assert_eq!(
            line_operand("#line")?,
            LineOperand::Malformed(LineOperandError::MissingLine)
        );
        Ok(())
    }

    #[test]
    fn reports_malformed_line_operand_filename() -> Result<(), String> {
        assert!(matches!(
            line_operand("#line 12 34")?,
            LineOperand::Malformed(LineOperandError::InvalidFilename(tokens))
                if matches!(tokens.as_slice(), [PpToken { kind: PpTokenKind::Number(value), .. }] if value == "34")
        ));
        assert!(matches!(
            line_operand("#line 12 <generated.c>")?,
            LineOperand::Malformed(LineOperandError::InvalidFilename(tokens))
                if matches!(tokens.first().map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == "<")
        ));
        Ok(())
    }

    #[test]
    fn parses_function_like_defines() -> Result<(), String> {
        assert!(matches!(
            directive("#define ADD(x, y) x + y")?,
            Directive::Define {
                name,
                def: MacroDef::Function { params, variadic: false, body },
            } if name == "ADD" && params == vec!["x", "y"] && !body.is_empty()
        ));
        assert!(matches!(
            directive("#define LOG(fmt, ...) fmt, __VA_ARGS__")?,
            Directive::Define {
                name,
                def: MacroDef::Function { params, variadic: true, body },
            } if name == "LOG" && params == vec!["fmt"] && !body.is_empty()
        ));
        Ok(())
    }

    #[test]
    fn rejects_duplicate_function_like_macro_parameters() -> Result<(), String> {
        for input in ["#define DUP(x, x) x", "#define DUP(x, y, x) x"] {
            assert!(
                directive(input).is_err(),
                "expected {input:?} to reject duplicate macro parameter names"
            );
        }
        Ok(())
    }

    #[test]
    fn rejects_reserved_function_like_macro_parameter_names() -> Result<(), String> {
        for input in [
            "#define BAD(__VA_ARGS__) __VA_ARGS__",
            "#define BAD(x, __VA_ARGS__) __VA_ARGS__",
            "#define BAD(__VA_ARGS__...) __VA_ARGS__",
            "#define BAD(__VA_OPT__) __VA_OPT__(x)",
            "#define BAD(x, __VA_OPT__) __VA_OPT__(x)",
        ] {
            assert!(
                directive(input).is_err(),
                "expected {input:?} to reject reserved macro parameter names"
            );
        }
        Ok(())
    }
}
