use std::collections::HashMap;

use super::lexer::lex;
use super::token::{PpToken, PpTokenKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MacroDef {
    Object(Vec<PpToken>),
    Function {
        params: Vec<String>,
        variadic: bool,
        body: Vec<PpToken>,
    },
}

pub type MacroTable = HashMap<String, MacroDef>;

pub trait MacroExpansionHooks {
    fn expand_unknown_ident(
        &mut self,
        _token: &PpToken,
        _name: &str,
    ) -> Result<Option<Vec<PpToken>>, String> {
        Ok(None)
    }
}

struct NoMacroExpansionHooks;

impl MacroExpansionHooks for NoMacroExpansionHooks {}

type InvocationArgs = (Vec<Vec<PpToken>>, usize);

pub fn expand_macros(tokens: &[PpToken], macros: &MacroTable) -> Result<Vec<PpToken>, String> {
    expand_macros_with_hooks(tokens, macros, &mut NoMacroExpansionHooks)
}

pub fn expand_macros_with_hooks(
    tokens: &[PpToken],
    macros: &MacroTable,
    hooks: &mut dyn MacroExpansionHooks,
) -> Result<Vec<PpToken>, String> {
    expand_macros_inner(tokens, macros, hooks, &mut Vec::new())
}

fn expand_macros_inner(
    tokens: &[PpToken],
    macros: &MacroTable,
    hooks: &mut dyn MacroExpansionHooks,
    disabled: &mut Vec<String>,
) -> Result<Vec<PpToken>, String> {
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let token = &tokens[index];
        let PpTokenKind::Ident(name) = &token.kind else {
            out.push(token.clone());
            index += 1;
            continue;
        };
        if disabled.iter().any(|disabled_name| disabled_name == name) {
            out.push(token.clone());
            index += 1;
            continue;
        }
        match macros.get(name) {
            Some(MacroDef::Object(replacement)) => {
                let replacement = paste_tokens(replacement)?;
                disabled.push(name.clone());
                out.extend(expand_macros_inner(&replacement, macros, hooks, disabled)?);
                disabled.pop();
                index += 1;
            }
            Some(MacroDef::Function {
                params,
                variadic,
                body,
            }) => {
                let Some((args, next_index)) = parse_invocation_args(tokens, index + 1)? else {
                    out.push(token.clone());
                    index += 1;
                    continue;
                };
                if (!variadic && args.len() != params.len())
                    || (*variadic && args.len() < params.len())
                {
                    out.push(token.clone());
                    index += 1;
                    continue;
                }
                let replacement =
                    substitute_function_macro(body, params, *variadic, &args, macros, hooks)?;
                disabled.push(name.clone());
                out.extend(expand_macros_inner(&replacement, macros, hooks, disabled)?);
                disabled.pop();
                index = next_index;
            }
            None => {
                if let Some(replacement) = hooks.expand_unknown_ident(token, name)? {
                    out.extend(expand_macros_inner(&replacement, macros, hooks, disabled)?);
                } else {
                    out.push(token.clone());
                }
                index += 1;
            }
        }
    }
    Ok(out)
}

fn parse_invocation_args(
    tokens: &[PpToken],
    start: usize,
) -> Result<Option<InvocationArgs>, String> {
    let mut index = skip_ws(tokens, start);
    if !is_punct(tokens.get(index), "(") {
        return Ok(None);
    }
    index += 1;
    let mut args = Vec::new();
    let mut current = Vec::new();
    let mut depth = 0usize;
    while index < tokens.len() {
        if is_punct(tokens.get(index), "(") {
            depth += 1;
            current.push(tokens[index].clone());
        } else if is_punct(tokens.get(index), ")") {
            if depth == 0 {
                if !current.is_empty() || !args.is_empty() {
                    args.push(trim_tokens(&current));
                }
                return Ok(Some((args, index + 1)));
            }
            depth -= 1;
            current.push(tokens[index].clone());
        } else if depth == 0 && is_punct(tokens.get(index), ",") {
            args.push(trim_tokens(&current));
            current.clear();
        } else {
            current.push(tokens[index].clone());
        }
        index += 1;
    }
    Err("missing ')' in function-like macro invocation".to_string())
}

fn substitute_function_macro(
    body: &[PpToken],
    params: &[String],
    variadic: bool,
    args: &[Vec<PpToken>],
    macros: &MacroTable,
    hooks: &mut dyn MacroExpansionHooks,
) -> Result<Vec<PpToken>, String> {
    let mut param_names = params.to_vec();
    let mut macro_args = args[..params.len()].to_vec();
    let variadic_args = if variadic {
        args[params.len()..].to_vec()
    } else {
        Vec::new()
    };
    let variadic_args_missing = variadic && args.len() == params.len();
    if variadic {
        param_names.push("__VA_ARGS__".to_string());
        macro_args.push(join_variadic_args(&variadic_args));
    }
    let has_variadic_args = variadic_args.iter().any(|arg| !arg.is_empty());
    let body = process_va_opt_tokens(body, has_variadic_args);
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < body.len() {
        if let Some((param_index, arg_index)) = comma_va_args_paste(&body, index, &param_names) {
            if !variadic_args_missing {
                out.push(body[index].clone());
                out.extend(expand_macros_with_hooks(
                    &macro_args[param_index],
                    macros,
                    hooks,
                )?);
            }
            index = arg_index + 1;
            continue;
        }
        if is_punct(body.get(index), "#") {
            let ident_index = skip_ws(&body, index + 1);
            if let Some((param_index, _name)) = param_at(&body, ident_index, &param_names) {
                out.push(stringify_arg(&body[index], &macro_args[param_index]));
                index = ident_index + 1;
                continue;
            }
        }
        if let Some((param_index, _name)) = param_at(&body, index, &param_names) {
            if adjacent_to_paste(&body, index) {
                out.extend(macro_args[param_index].clone());
            } else {
                out.extend(expand_macros_with_hooks(
                    &macro_args[param_index],
                    macros,
                    hooks,
                )?);
            }
            index += 1;
        } else {
            out.push(body[index].clone());
            index += 1;
        }
    }
    paste_tokens(&out)
}

fn process_va_opt_tokens(body: &[PpToken], has_variadic_args: bool) -> Vec<PpToken> {
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < body.len() {
        if is_ident(body.get(index), "__VA_OPT__") {
            let open = skip_ws(body, index + 1);
            if is_punct(body.get(open), "(") {
                if let Some((inside, next)) = collect_balanced(body, open) {
                    if has_variadic_args {
                        out.extend(inside);
                    }
                    index = next;
                    continue;
                }
            }
        }
        out.push(body[index].clone());
        index += 1;
    }
    out
}

fn collect_balanced(tokens: &[PpToken], open_index: usize) -> Option<(Vec<PpToken>, usize)> {
    let mut depth = 0usize;
    let mut out = Vec::new();
    for (index, token) in tokens.iter().enumerate().skip(open_index + 1) {
        if is_punct(Some(token), "(") {
            depth += 1;
            out.push(token.clone());
        } else if is_punct(Some(token), ")") {
            if depth == 0 {
                return Some((out, index + 1));
            }
            depth -= 1;
            out.push(token.clone());
        } else {
            out.push(token.clone());
        }
    }
    None
}

fn paste_tokens(tokens: &[PpToken]) -> Result<Vec<PpToken>, String> {
    let mut out: Vec<PpToken> = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        if is_punct(tokens.get(index), "##") {
            while matches!(
                out.last().map(|token| &token.kind),
                Some(PpTokenKind::Whitespace(_))
            ) {
                out.pop();
            }
            let Some(left) = out.pop() else {
                index += 1;
                continue;
            };
            let right_index = skip_ws(tokens, index + 1);
            let Some(right) = tokens.get(right_index) else {
                out.push(left);
                break;
            };
            let pasted = format!("{}{}", left.text(), right.text());
            let mut pasted_tokens = lex(&pasted)?;
            if pasted_tokens.len() != 1 || pasted_tokens[0].text() != pasted {
                return Err(format!("invalid token paste: {}", pasted));
            }
            out.append(&mut pasted_tokens);
            index = right_index + 1;
        } else {
            out.push(tokens[index].clone());
            index += 1;
        }
    }
    Ok(out)
}

fn comma_va_args_paste(
    tokens: &[PpToken],
    index: usize,
    params: &[String],
) -> Option<(usize, usize)> {
    if !is_punct(tokens.get(index), ",") {
        return None;
    }
    let paste_index = skip_ws(tokens, index + 1);
    if !is_punct(tokens.get(paste_index), "##") {
        return None;
    }
    let arg_index = skip_ws(tokens, paste_index + 1);
    match param_at(tokens, arg_index, params) {
        Some((param_index, "__VA_ARGS__")) => Some((param_index, arg_index)),
        _ => None,
    }
}

fn stringify_arg(anchor: &PpToken, arg: &[PpToken]) -> PpToken {
    let mut text = String::new();
    let mut need_space = false;
    for token in arg {
        if matches!(
            token.kind,
            PpTokenKind::Whitespace(_) | PpTokenKind::Newline(_)
        ) {
            if !text.is_empty() {
                need_space = true;
            }
            continue;
        }
        if need_space {
            text.push(' ');
            need_space = false;
        }
        text.push_str(token.text());
    }
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    anchor.clone_with_text(PpTokenKind::StringLit(format!("\"{}\"", escaped)))
}

fn join_variadic_args(args: &[Vec<PpToken>]) -> Vec<PpToken> {
    let mut out = Vec::new();
    for (index, arg) in args.iter().enumerate() {
        if index > 0 {
            if let Some(anchor) = arg.first().or_else(|| out.last()).cloned() {
                out.push(anchor.clone_with_text(PpTokenKind::Punct(",".to_string())));
                out.push(anchor.clone_with_text(PpTokenKind::Whitespace(" ".to_string())));
            }
        }
        out.extend(arg.clone());
    }
    out
}

fn trim_tokens(tokens: &[PpToken]) -> Vec<PpToken> {
    let start = tokens
        .iter()
        .position(|token| {
            !matches!(
                token.kind,
                PpTokenKind::Whitespace(_) | PpTokenKind::Newline(_)
            )
        })
        .unwrap_or(tokens.len());
    let end = tokens
        .iter()
        .rposition(|token| {
            !matches!(
                token.kind,
                PpTokenKind::Whitespace(_) | PpTokenKind::Newline(_)
            )
        })
        .map(|index| index + 1)
        .unwrap_or(start);
    tokens[start..end].to_vec()
}

fn skip_ws(tokens: &[PpToken], mut index: usize) -> usize {
    while matches!(
        tokens.get(index).map(|token| &token.kind),
        Some(PpTokenKind::Whitespace(_) | PpTokenKind::Newline(_))
    ) {
        index += 1;
    }
    index
}

fn param_at<'a>(
    tokens: &'a [PpToken],
    index: usize,
    params: &'a [String],
) -> Option<(usize, &'a str)> {
    let PpTokenKind::Ident(name) = &tokens.get(index)?.kind else {
        return None;
    };
    params
        .iter()
        .position(|param| param == name)
        .map(|param_index| (param_index, name.as_str()))
}

fn adjacent_to_paste(tokens: &[PpToken], index: usize) -> bool {
    let previous = tokens[..index]
        .iter()
        .rposition(|token| !matches!(token.kind, PpTokenKind::Whitespace(_)))
        .and_then(|index| tokens.get(index));
    let next = tokens[index + 1..]
        .iter()
        .position(|token| !matches!(token.kind, PpTokenKind::Whitespace(_)))
        .and_then(|offset| tokens.get(index + 1 + offset));
    is_punct(previous, "##") || is_punct(next, "##")
}

fn is_ident(token: Option<&PpToken>, expected: &str) -> bool {
    matches!(token.map(|token| &token.kind), Some(PpTokenKind::Ident(name)) if name == expected)
}

fn is_punct(token: Option<&PpToken>, expected: &str) -> bool {
    matches!(token.map(|token| &token.kind), Some(PpTokenKind::Punct(value)) if value == expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(tokens: &[PpToken]) -> String {
        tokens.iter().map(PpToken::text).collect()
    }

    struct TestBuiltinHooks {
        file: String,
        line: usize,
        counter: usize,
    }

    impl MacroExpansionHooks for TestBuiltinHooks {
        fn expand_unknown_ident(
            &mut self,
            token: &PpToken,
            name: &str,
        ) -> Result<Option<Vec<PpToken>>, String> {
            let kind = match name {
                "__LINE__" => PpTokenKind::Number(self.line.to_string()),
                "__FILE__" => PpTokenKind::StringLit(format!("\"{}\"", self.file)),
                "__COUNTER__" => {
                    let counter = self.counter;
                    self.counter += 1;
                    PpTokenKind::Number(counter.to_string())
                }
                _ => return Ok(None),
            };
            Ok(Some(vec![token.clone_with_text(kind)]))
        }
    }

    #[test]
    fn expands_object_macros() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("A".to_string(), MacroDef::Object(lex("42")?));
        assert_eq!(text(&expand_macros(&lex("A")?, &macros)?), "42");
        Ok(())
    }

    #[test]
    fn rescans_object_replacements() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("A".to_string(), MacroDef::Object(lex("B")?));
        macros.insert("B".to_string(), MacroDef::Object(lex("42")?));
        assert_eq!(text(&expand_macros(&lex("A")?, &macros)?), "42");
        Ok(())
    }

    #[test]
    fn object_macro_token_paste_is_rescanned() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("KW".to_string(), MacroDef::Object(lex("in ## t")?));
        macros.insert("int".to_string(), MacroDef::Object(lex("long")?));
        assert_eq!(
            text(&expand_macros(&lex("KW value")?, &macros)?),
            "long value"
        );
        Ok(())
    }

    #[test]
    fn leaves_self_recursion_disabled() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("A".to_string(), MacroDef::Object(lex("A")?));
        assert_eq!(text(&expand_macros(&lex("A")?, &macros)?), "A");
        Ok(())
    }

    #[test]
    fn leaves_indirect_object_recursion_disabled() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("A".to_string(), MacroDef::Object(lex("B")?));
        macros.insert("B".to_string(), MacroDef::Object(lex("A")?));
        assert_eq!(text(&expand_macros(&lex("A")?, &macros)?), "A");
        Ok(())
    }

    #[test]
    fn leaves_indirect_function_recursion_disabled() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "A".to_string(),
            MacroDef::Function {
                params: Vec::new(),
                variadic: false,
                body: lex("B()")?,
            },
        );
        macros.insert(
            "B".to_string(),
            MacroDef::Function {
                params: Vec::new(),
                variadic: false,
                body: lex("A()")?,
            },
        );
        assert_eq!(text(&expand_macros(&lex("A()")?, &macros)?), "A()");
        Ok(())
    }

    #[test]
    fn expands_function_macros_with_argument_prescan() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("A".to_string(), MacroDef::Object(lex("40")?));
        macros.insert(
            "ADD".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string(), "y".to_string()],
                variadic: false,
                body: lex("x + y")?,
            },
        );
        assert_eq!(text(&expand_macros(&lex("ADD(A, 2)")?, &macros)?), "40 + 2");
        Ok(())
    }

    #[test]
    fn rejects_unterminated_function_macro_invocation() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "ADD".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string(), "y".to_string()],
                variadic: false,
                body: lex("x + y")?,
            },
        );
        assert!(expand_macros(&lex("ADD(1, 2")?, &macros).is_err());
        Ok(())
    }

    #[test]
    fn expands_function_macros_across_newline_before_arguments() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "EMPTY".to_string(),
            MacroDef::Function {
                params: Vec::new(),
                variadic: true,
                body: Vec::new(),
            },
        );
        assert_eq!(
            text(&expand_macros(
                &lex("value EMPTY\n(ignored)\n= 1")?,
                &macros
            )?),
            "value \n= 1"
        );
        Ok(())
    }

    #[test]
    fn stringifies_raw_arguments() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("A".to_string(), MacroDef::Object(lex("40")?));
        macros.insert(
            "STR".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string()],
                variadic: false,
                body: lex("#x")?,
            },
        );
        assert_eq!(
            text(&expand_macros(&lex("STR(A + 2)")?, &macros)?),
            "\"A + 2\""
        );
        Ok(())
    }

    #[test]
    fn stringification_normalizes_whitespace_and_comments() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "STR".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string()],
                variadic: false,
                body: lex("# x")?,
            },
        );
        assert_eq!(
            text(&expand_macros(
                &lex("STR(  a/* hidden */\t+\n b  )")?,
                &macros
            )?),
            "\"a + b\""
        );
        Ok(())
    }

    #[test]
    fn stringification_escapes_literals_after_whitespace_normalization() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "STR".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string()],
                variadic: false,
                body: lex("#x")?,
            },
        );
        assert_eq!(
            text(&expand_macros(&lex("STR(\"a\\\\b\" '\"')")?, &macros)?),
            "\"\\\"a\\\\\\\\b\\\" '\\\"'\""
        );
        Ok(())
    }

    #[test]
    fn pastes_raw_arguments_then_rescans() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert("xy".to_string(), MacroDef::Object(lex("42")?));
        macros.insert(
            "CAT".to_string(),
            MacroDef::Function {
                params: vec!["a".to_string(), "b".to_string()],
                variadic: false,
                body: lex("a ## b")?,
            },
        );
        assert_eq!(text(&expand_macros(&lex("CAT(x, y)")?, &macros)?), "42");
        Ok(())
    }

    #[test]
    fn rejects_token_paste_that_does_not_form_single_token() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "BAD".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string()],
                variadic: false,
                body: lex("x ## +")?,
            },
        );
        assert!(expand_macros(&lex("BAD(a)")?, &macros).is_err());
        Ok(())
    }

    #[test]
    fn handles_va_opt() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "CALL".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string()],
                variadic: true,
                body: lex("f(x __VA_OPT__(,) __VA_ARGS__)")?,
            },
        );
        assert_eq!(text(&expand_macros(&lex("CALL(1)")?, &macros)?), "f(1  )");
        assert_eq!(
            text(&expand_macros(&lex("CALL(1, 2)")?, &macros)?),
            "f(1 , 2)"
        );
        Ok(())
    }

    #[test]
    fn elides_comma_before_missing_va_args_with_gnu_paste() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "LOG".to_string(),
            MacroDef::Function {
                params: vec!["fmt".to_string()],
                variadic: true,
                body: lex("printf(fmt, ## __VA_ARGS__)")?,
            },
        );
        assert_eq!(
            text(&expand_macros(&lex("LOG(\"ok\")")?, &macros)?),
            "printf(\"ok\")"
        );
        assert_eq!(
            text(&expand_macros(&lex("LOG(\"%d\", 7)")?, &macros)?),
            "printf(\"%d\",7)"
        );
        Ok(())
    }

    #[test]
    fn keeps_comma_for_present_but_empty_va_args_with_gnu_paste() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "LOG".to_string(),
            MacroDef::Function {
                params: vec!["fmt".to_string()],
                variadic: true,
                body: lex("printf(fmt, ## __VA_ARGS__)")?,
            },
        );
        assert_eq!(
            text(&expand_macros(&lex("LOG(\"ok\",)")?, &macros)?),
            "printf(\"ok\",)"
        );
        Ok(())
    }

    #[test]
    fn va_opt_handles_nested_parentheses_and_empty_variadic_argument() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "WRAP".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string()],
                variadic: true,
                body: lex("f(x __VA_OPT__(, g(__VA_ARGS__)))")?,
            },
        );
        assert_eq!(text(&expand_macros(&lex("WRAP(1)")?, &macros)?), "f(1 )");
        assert_eq!(text(&expand_macros(&lex("WRAP(1,)")?, &macros)?), "f(1 )");
        assert_eq!(
            text(&expand_macros(&lex("WRAP(1, h(2, 3))")?, &macros)?),
            "f(1 , g(h(2, 3)))"
        );
        Ok(())
    }

    #[test]
    fn va_opt_expands_when_any_variadic_argument_has_tokens() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "M".to_string(),
            MacroDef::Function {
                params: Vec::new(),
                variadic: true,
                body: lex("a __VA_OPT__(+ __VA_ARGS__)")?,
            },
        );
        assert_eq!(text(&expand_macros(&lex("M()")?, &macros)?), "a ");
        assert_eq!(text(&expand_macros(&lex("M( )")?, &macros)?), "a ");
        assert_eq!(text(&expand_macros(&lex("M(1, 2)")?, &macros)?), "a + 1, 2");
        Ok(())
    }

    #[test]
    fn hook_expands_unknown_builtin_like_identifiers() -> Result<(), String> {
        let macros = MacroTable::new();
        let mut hooks = TestBuiltinHooks {
            file: "main.c".to_string(),
            line: 27,
            counter: 0,
        };
        assert_eq!(
            text(&expand_macros_with_hooks(
                &lex("__LINE__ __FILE__ __COUNTER__ __COUNTER__ unknown")?,
                &macros,
                &mut hooks,
            )?),
            "27 \"main.c\" 0 1 unknown"
        );
        Ok(())
    }

    #[test]
    fn hook_expands_during_object_macro_rescan() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "LOCATION".to_string(),
            MacroDef::Object(lex("__FILE__:__LINE__")?),
        );
        let mut hooks = TestBuiltinHooks {
            file: "nested.c".to_string(),
            line: 9,
            counter: 0,
        };
        assert_eq!(
            text(&expand_macros_with_hooks(
                &lex("LOCATION")?,
                &macros,
                &mut hooks,
            )?),
            "\"nested.c\":9"
        );
        Ok(())
    }

    #[test]
    fn hook_expands_during_function_argument_prescan() -> Result<(), String> {
        let mut macros = MacroTable::new();
        macros.insert(
            "ID".to_string(),
            MacroDef::Function {
                params: vec!["x".to_string()],
                variadic: false,
                body: lex("x")?,
            },
        );
        let mut hooks = TestBuiltinHooks {
            file: "unused.c".to_string(),
            line: 41,
            counter: 0,
        };
        assert_eq!(
            text(&expand_macros_with_hooks(
                &lex("ID(__COUNTER__) ID(__COUNTER__)")?,
                &macros,
                &mut hooks,
            )?),
            "0 1"
        );
        Ok(())
    }
}
