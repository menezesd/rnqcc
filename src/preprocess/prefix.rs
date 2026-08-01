// Character-level preprocessing: ident detection, line splicing, comment stripping,
// and C string escaping. These operate on raw source text before tokenization.

pub fn is_ident_start(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_start(ch)
}

pub fn is_ident_continue(ch: char) -> bool {
    ch == '_' || unicode_ident::is_xid_continue(ch)
}

/// Splits physical lines joined by backslash-newline (or backslash-CR, backslash-CR-LF).
pub fn splice_continued_lines(source: &str) -> String {
    let mut out = String::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.peek().copied() {
                Some('\n') => {
                    chars.next();
                    continue;
                }
                Some('\r') => {
                    chars.next();
                    if matches!(chars.peek(), Some('\n')) {
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

/// Strips C comments (// and /* */) from source text, respecting string/char literals.
pub fn strip_comments(source: &str) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\'' if out
                .chars()
                .last()
                .is_some_and(|previous| previous.is_ascii_hexdigit())
                && chars.peek().is_some_and(|next| next.is_ascii_hexdigit()) =>
            {
                out.push(ch);
            }
            '"' | '\'' => {
                out.push(ch);
                let quote = ch;
                let mut escaped = false;
                for inner in chars.by_ref() {
                    out.push(inner);
                    if escaped {
                        escaped = false;
                    } else if inner == '\\' {
                        escaped = true;
                    } else if inner == quote {
                        break;
                    }
                }
            }
            '/' if matches!(chars.peek(), Some('/')) => {
                chars.next();
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if matches!(chars.peek(), Some('*')) => {
                chars.next();
                let mut closed = false;
                let mut previous = '\0';
                for inner in chars.by_ref() {
                    if inner == '\n' {
                        out.push('\n');
                    } else if previous == '*' && inner == '/' {
                        closed = true;
                        break;
                    }
                    previous = inner;
                }
                if !closed {
                    return Err("unterminated block comment in preprocessor input".to_string());
                }
                out.push(' ');
            }
            _ => out.push(ch),
        }
    }
    Ok(out)
}

/// Escape a string for use as a C string literal (for __FILE__ etc.).
pub fn escape_c_string(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(ch),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_comments_preserves_c23_digit_separators() -> Result<(), String> {
        assert_eq!(
            strip_comments("int value = 0xca'fe /* ignored */;\n")?,
            "int value = 0xca'fe  ;\n"
        );
        Ok(())
    }
}
