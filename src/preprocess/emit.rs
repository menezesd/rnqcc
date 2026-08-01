use super::token::{PpToken, PpTokenKind};

pub fn emit_tokens(tokens: &[PpToken]) -> String {
    let mut out = String::with_capacity(tokens.iter().map(|token| token.text().len()).sum());
    let mut previous: Option<&PpToken> = None;
    let mut previous_emitted_whitespace = false;
    for token in tokens {
        if should_insert_space(previous, token, previous_emitted_whitespace) {
            out.push(' ');
        }
        let text = token.text();
        out.push_str(text);
        previous_emitted_whitespace = text.chars().last().is_some_and(char::is_whitespace);
        previous = Some(token);
    }
    out
}

fn should_insert_space(
    previous: Option<&PpToken>,
    current: &PpToken,
    previous_emitted_whitespace: bool,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if previous_emitted_whitespace {
        return false;
    }
    if matches!(
        (&previous.kind, &current.kind),
        (PpTokenKind::Whitespace(_), _)
            | (PpTokenKind::Newline(_), _)
            | (_, PpTokenKind::Whitespace(_))
            | (_, PpTokenKind::Newline(_))
    ) {
        return false;
    }
    tokens_would_merge(previous, current)
}

fn tokens_would_merge(previous: &PpToken, current: &PpToken) -> bool {
    match (&previous.kind, &current.kind) {
        (PpTokenKind::Ident(left), PpTokenKind::Ident(_) | PpTokenKind::Number(_)) => {
            can_continue_identifier(left)
        }
        (PpTokenKind::Number(left), PpTokenKind::Ident(_) | PpTokenKind::Number(_)) => {
            can_continue_pp_number(left)
        }
        (PpTokenKind::Number(left), PpTokenKind::Punct(right)) => {
            right == "." || matches!(right.as_str(), "+" | "-") && ends_pp_exponent(left)
        }
        (PpTokenKind::Punct(left), PpTokenKind::Number(_)) => left == ".",
        (PpTokenKind::Ident(left), PpTokenKind::StringLit(_) | PpTokenKind::CharLit(_)) => {
            matches!(left.as_str(), "L" | "u" | "U" | "u8")
        }
        (PpTokenKind::Punct(left), PpTokenKind::Punct(right)) => {
            punctuators_or_comments_would_merge(left, right)
        }
        _ => false,
    }
}

fn can_continue_identifier(left: &str) -> bool {
    left.chars()
        .last()
        .is_some_and(super::lexer::is_ident_continue)
}

fn can_continue_pp_number(left: &str) -> bool {
    left.chars()
        .last()
        .is_some_and(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '\''))
}

fn ends_pp_exponent(text: &str) -> bool {
    matches!(text.chars().last(), Some('e' | 'E' | 'p' | 'P'))
}

fn punctuators_or_comments_would_merge(left: &str, right: &str) -> bool {
    matches!(
        (left, right),
        ("+", "+")
            | ("+", "=")
            | ("-", "-")
            | ("-", ">")
            | ("-", "=")
            | ("<", "<")
            | ("<", "=")
            | ("<", ":")
            | ("<", "%")
            | ("<<", "=")
            | (">", ">")
            | (">", "=")
            | (">>", "=")
            | ("=", "=")
            | ("!", "=")
            | ("&", "&")
            | ("&", "=")
            | ("|", "|")
            | ("|", "=")
            | ("*", "=")
            | ("/", "/")
            | ("/", "*")
            | ("/", "=")
            | ("%", "=")
            | ("%", ":")
            | ("%:", "%:")
            | ("%:", "%")
            | ("%>", "=")
            | ("^", "=")
            | (":", ">")
            | (".", ".")
            | ("..", ".")
            | ("#", "#")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preprocess::lexer::lex;

    #[test]
    fn preserves_existing_whitespace() -> Result<(), String> {
        assert_eq!(emit_tokens(&lex("int  x\n")?), "int  x\n");
        Ok(())
    }

    #[test]
    fn inserts_space_when_tokens_would_merge() -> Result<(), String> {
        let mut left = lex("foo")?;
        let right = lex("bar")?;
        left.extend(right);
        assert_eq!(emit_tokens(&left), "foo bar");
        Ok(())
    }

    #[test]
    fn inserts_space_when_punctuators_would_merge() -> Result<(), String> {
        for (input, expected) in [
            ("+ +", "+ +"),
            ("- >", "- >"),
            ("< :", "< :"),
            ("# #", "# #"),
            ("/ /", "/ /"),
            ("/ *", "/ *"),
        ] {
            let tokens: Vec<_> = lex(input)?
                .into_iter()
                .filter(|token| !matches!(token.kind, PpTokenKind::Whitespace(_)))
                .collect();
            assert_eq!(emit_tokens(&tokens), expected);
        }
        Ok(())
    }

    #[test]
    fn inserts_space_when_punctuator_would_extend_number() -> Result<(), String> {
        let tokens: Vec<_> = lex("1e + 2")?
            .into_iter()
            .filter(|token| !matches!(token.kind, PpTokenKind::Whitespace(_)))
            .collect();
        assert_eq!(emit_tokens(&tokens), "1e +2");
        Ok(())
    }

    #[test]
    fn does_not_space_punctuation() -> Result<(), String> {
        assert_eq!(emit_tokens(&lex("f(1+2)")?), "f(1+2)");
        Ok(())
    }
}
