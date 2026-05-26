use super::directive::{parse_include_operand, IncludeOperand};
use super::token::{PpToken, PpTokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateOperand {
    Defined {
        name: String,
    },
    HasInclude {
        operand: IncludeOperand,
        include_next: bool,
    },
    HasBuiltin {
        name: String,
    },
    HasAttribute {
        name: String,
    },
    HasCAttribute {
        name: String,
    },
    HasDeclspecAttribute {
        name: String,
    },
    HasFeature {
        name: String,
    },
    HasExtension {
        name: String,
    },
    HasWarning {
        name: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedPredicateOperand {
    pub operand: PredicateOperand,
    pub next_index: usize,
}

pub fn parse_predicate_operand(tokens: &[PpToken], start: usize) -> Option<ParsedPredicateOperand> {
    match ident_text(tokens.get(start))? {
        "defined" => parse_defined_operand(tokens, start),
        "__has_include" => parse_has_include_operand(tokens, start, false),
        "__has_include_next" => parse_has_include_operand(tokens, start, true),
        "__has_builtin" => {
            parse_identifier_call_operand(tokens, start).map(|parsed| ParsedPredicateOperand {
                operand: PredicateOperand::HasBuiltin { name: parsed.name },
                next_index: parsed.next_index,
            })
        }
        "__has_attribute" => {
            parse_identifier_call_operand(tokens, start).map(|parsed| ParsedPredicateOperand {
                operand: PredicateOperand::HasAttribute { name: parsed.name },
                next_index: parsed.next_index,
            })
        }
        "__has_c_attribute" => {
            parse_identifier_call_operand(tokens, start).map(|parsed| ParsedPredicateOperand {
                operand: PredicateOperand::HasCAttribute { name: parsed.name },
                next_index: parsed.next_index,
            })
        }
        "__has_declspec_attribute" => {
            parse_identifier_call_operand(tokens, start).map(|parsed| ParsedPredicateOperand {
                operand: PredicateOperand::HasDeclspecAttribute { name: parsed.name },
                next_index: parsed.next_index,
            })
        }
        "__has_feature" => {
            parse_identifier_call_operand(tokens, start).map(|parsed| ParsedPredicateOperand {
                operand: PredicateOperand::HasFeature { name: parsed.name },
                next_index: parsed.next_index,
            })
        }
        "__has_extension" => {
            parse_identifier_call_operand(tokens, start).map(|parsed| ParsedPredicateOperand {
                operand: PredicateOperand::HasExtension { name: parsed.name },
                next_index: parsed.next_index,
            })
        }
        "__has_warning" => {
            parse_string_call_operand(tokens, start).map(|parsed| ParsedPredicateOperand {
                operand: PredicateOperand::HasWarning { name: parsed.name },
                next_index: parsed.next_index,
            })
        }
        _ => None,
    }
}

struct ParsedIdentifierCallOperand {
    name: String,
    next_index: usize,
}

fn parse_identifier_call_operand(
    tokens: &[PpToken],
    ident_index: usize,
) -> Option<ParsedIdentifierCallOperand> {
    let open = skip_ws(tokens, ident_index + 1);
    if !is_punct(tokens.get(open), "(") {
        return None;
    }
    let name_index = skip_ws(tokens, open + 1);
    let name = ident_text(tokens.get(name_index))?;
    let close = skip_ws(tokens, name_index + 1);
    if !is_punct(tokens.get(close), ")") {
        return None;
    }
    Some(ParsedIdentifierCallOperand {
        name: name.to_string(),
        next_index: close + 1,
    })
}

fn parse_string_call_operand(
    tokens: &[PpToken],
    ident_index: usize,
) -> Option<ParsedIdentifierCallOperand> {
    let open = skip_ws(tokens, ident_index + 1);
    if !is_punct(tokens.get(open), "(") {
        return None;
    }
    let name_index = skip_ws(tokens, open + 1);
    let name = string_text(tokens.get(name_index))?;
    let close = skip_ws(tokens, name_index + 1);
    if !is_punct(tokens.get(close), ")") {
        return None;
    }
    Some(ParsedIdentifierCallOperand {
        name: name.to_string(),
        next_index: close + 1,
    })
}

pub fn parse_predicate_operand_all(tokens: &[PpToken]) -> Option<PredicateOperand> {
    let parsed = parse_predicate_operand(tokens, skip_ws(tokens, 0))?;
    let end = skip_ws(tokens, parsed.next_index);
    (end == tokens.len()).then_some(parsed.operand)
}

fn parse_defined_operand(
    tokens: &[PpToken],
    defined_index: usize,
) -> Option<ParsedPredicateOperand> {
    let after_defined = skip_ws(tokens, defined_index + 1);
    if is_punct(tokens.get(after_defined), "(") {
        let name_index = skip_ws(tokens, after_defined + 1);
        let name = ident_text(tokens.get(name_index))?;
        let close = skip_ws(tokens, name_index + 1);
        if !is_punct(tokens.get(close), ")") {
            return None;
        }
        return Some(ParsedPredicateOperand {
            operand: PredicateOperand::Defined {
                name: name.to_string(),
            },
            next_index: close + 1,
        });
    }

    let name = ident_text(tokens.get(after_defined))?;
    Some(ParsedPredicateOperand {
        operand: PredicateOperand::Defined {
            name: name.to_string(),
        },
        next_index: after_defined + 1,
    })
}

fn parse_has_include_operand(
    tokens: &[PpToken],
    ident_index: usize,
    include_next: bool,
) -> Option<ParsedPredicateOperand> {
    let open = skip_ws(tokens, ident_index + 1);
    if !is_punct(tokens.get(open), "(") {
        return None;
    }
    let close = find_matching_paren(tokens, open)?;
    Some(ParsedPredicateOperand {
        operand: PredicateOperand::HasInclude {
            operand: parse_include_operand(&tokens[open + 1..close]),
            include_next,
        },
        next_index: close + 1,
    })
}

fn find_matching_paren(tokens: &[PpToken], open_index: usize) -> Option<usize> {
    if !is_punct(tokens.get(open_index), "(") {
        return None;
    }
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open_index + 1) {
        if is_punct(Some(token), "(") {
            depth += 1;
        } else if is_punct(Some(token), ")") {
            if depth == 0 {
                return Some(index);
            }
            depth -= 1;
        }
    }
    None
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

fn ident_text(token: Option<&PpToken>) -> Option<&str> {
    match token.map(|token| &token.kind) {
        Some(PpTokenKind::Ident(value)) => Some(value),
        _ => None,
    }
}

fn string_text(token: Option<&PpToken>) -> Option<&str> {
    match token.map(|token| &token.kind) {
        Some(PpTokenKind::StringLit(value)) => value.strip_prefix('"')?.strip_suffix('"'),
        _ => None,
    }
}

fn is_punct(token: Option<&PpToken>, expected: &str) -> bool {
    matches!(
        token.map(|token| &token.kind),
        Some(PpTokenKind::Punct(value)) if value == expected
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocess::directive::HeaderName;
    use crate::preprocess::lexer::lex;

    fn operand(input: &str) -> Result<PredicateOperand, String> {
        parse_predicate_operand_all(&lex(input)?)
            .ok_or_else(|| format!("expected predicate operand for {input:?}"))
    }

    #[test]
    fn recognizes_defined_identifier_operand() -> Result<(), String> {
        assert_eq!(
            operand("defined FEATURE")?,
            PredicateOperand::Defined {
                name: "FEATURE".to_string(),
            }
        );
        Ok(())
    }

    #[test]
    fn recognizes_parenthesized_defined_operand() -> Result<(), String> {
        assert_eq!(
            operand("defined ( FEATURE )")?,
            PredicateOperand::Defined {
                name: "FEATURE".to_string(),
            }
        );
        Ok(())
    }

    #[test]
    fn rejects_malformed_defined_operands() -> Result<(), String> {
        assert!(parse_predicate_operand_all(&lex("defined 123")?).is_none());
        assert!(parse_predicate_operand_all(&lex("defined(FEATURE")?).is_none());
        assert!(parse_predicate_operand_all(&lex("defined(FEATURE EXTRA)")?).is_none());
        Ok(())
    }

    #[test]
    fn recognizes_has_include_literal_operands() -> Result<(), String> {
        assert_eq!(
            operand("__has_include(<stdio.h>)")?,
            PredicateOperand::HasInclude {
                operand: IncludeOperand::Literal(HeaderName::Angled("stdio.h".to_string())),
                include_next: false,
            }
        );
        assert_eq!(
            operand("__has_include ( \"local.h\" )")?,
            PredicateOperand::HasInclude {
                operand: IncludeOperand::Literal(HeaderName::Quoted("local.h".to_string())),
                include_next: false,
            }
        );
        Ok(())
    }

    #[test]
    fn recognizes_has_include_next_literal_operand() -> Result<(), String> {
        assert_eq!(
            operand("__has_include_next(<next.h>)")?,
            PredicateOperand::HasInclude {
                operand: IncludeOperand::Literal(HeaderName::Angled("next.h".to_string())),
                include_next: true,
            }
        );
        Ok(())
    }

    #[test]
    fn recognizes_has_builtin_identifier_operand() -> Result<(), String> {
        assert_eq!(
            operand("__has_builtin(__builtin_expect)")?,
            PredicateOperand::HasBuiltin {
                name: "__builtin_expect".to_string()
            }
        );
        assert!(parse_predicate_operand_all(&lex("__has_builtin(123)")?).is_none());
        assert!(parse_predicate_operand_all(&lex("__has_builtin(__builtin_expect, 1)")?).is_none());
        Ok(())
    }

    #[test]
    fn recognizes_has_c_attribute_identifier_operand() -> Result<(), String> {
        assert_eq!(
            operand("__has_c_attribute(fallthrough)")?,
            PredicateOperand::HasCAttribute {
                name: "fallthrough".to_string()
            }
        );
        assert!(parse_predicate_operand_all(&lex("__has_c_attribute(123)")?).is_none());
        assert!(parse_predicate_operand_all(&lex("__has_c_attribute(fallthrough, 1)")?).is_none());
        Ok(())
    }

    #[test]
    fn recognizes_has_warning_string_operand() -> Result<(), String> {
        assert_eq!(
            operand("__has_warning(\"-Wunreachable\")")?,
            PredicateOperand::HasWarning {
                name: "-Wunreachable".to_string()
            }
        );
        assert!(parse_predicate_operand_all(&lex("__has_warning(-Wunreachable)")?).is_none());
        assert!(
            parse_predicate_operand_all(&lex("__has_warning(\"-Wunreachable\", 1)")?).is_none()
        );
        Ok(())
    }

    #[test]
    fn preserves_has_include_macro_operands_as_tokens() -> Result<(), String> {
        assert!(matches!(
            operand("__has_include(HEADER_NAME)")?,
            PredicateOperand::HasInclude {
                operand: IncludeOperand::Tokens(tokens),
                include_next: false,
            } if matches!(tokens.as_slice(), [PpToken { kind: PpTokenKind::Ident(name), .. }] if name == "HEADER_NAME")
        ));
        Ok(())
    }

    #[test]
    fn reports_consumed_token_count_for_embedded_operands() -> Result<(), String> {
        let tokens = lex("defined FEATURE && 1")?;
        let parsed = parse_predicate_operand(&tokens, 0).ok_or("expected parsed operand")?;
        assert_eq!(
            parsed.operand,
            PredicateOperand::Defined {
                name: "FEATURE".to_string(),
            }
        );
        assert!(matches!(
            tokens.get(skip_ws(&tokens, parsed.next_index)).map(|token| &token.kind),
            Some(PpTokenKind::Punct(op)) if op == "&&"
        ));
        Ok(())
    }

    #[test]
    fn parses_all_with_leading_whitespace_without_embedded_overconsume() -> Result<(), String> {
        assert_eq!(
            operand(" \t defined(FEATURE)")?,
            PredicateOperand::Defined {
                name: "FEATURE".to_string(),
            }
        );

        let tokens = lex("0 || __has_include(<stdio.h>)")?;
        assert!(parse_predicate_operand(&tokens, 3).is_none());

        let parsed = parse_predicate_operand(&tokens, 4).ok_or("expected parsed operand")?;
        assert_eq!(
            parsed.operand,
            PredicateOperand::HasInclude {
                operand: IncludeOperand::Literal(HeaderName::Angled("stdio.h".to_string())),
                include_next: false,
            }
        );
        assert_eq!(parsed.next_index, tokens.len());
        Ok(())
    }
}
