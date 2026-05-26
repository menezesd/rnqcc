use super::lexer::lex;
use super::token::{PpToken, PpTokenKind};

pub fn emit_tokens(tokens: &[PpToken]) -> String {
    let mut out = String::new();
    let mut previous: Option<&PpToken> = None;
    for token in tokens {
        if should_insert_space(previous, token, &out) {
            out.push(' ');
        }
        out.push_str(token.text());
        previous = Some(token);
    }
    out
}

fn should_insert_space(previous: Option<&PpToken>, current: &PpToken, out: &str) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if out.ends_with(char::is_whitespace) {
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
    let joined = format!("{}{}", previous.text(), current.text());
    match lex(&joined) {
        Ok(tokens) => tokens.len() == 1 && tokens[0].text() == joined,
        Err(_) => true,
    }
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
