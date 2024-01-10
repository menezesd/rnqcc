use crate::types::*;

#[derive(Debug, PartialEq)]
enum Expected {
    Tok(Token),
    Name(String),
}

fn pp_expected(expected: &Expected) -> String {
    match expected {
        Expected::Tok(tk) => format!("{:?}", tk),
        Expected::Name(s) => s.clone(),
    }
}

#[derive(Debug)]
pub struct ParseError(String);

fn raise_error(expected: Expected, actual: &Token) -> ParseError {
    let msg = format!("Expected {} but found {:?}", pp_expected(&expected), actual);
    ParseError(msg)
}

fn expect(expected: Token, tokens: &mut std::slice::Iter<Token>) -> Result<(), ParseError> {
    match tokens.next() {
        Some(actual_token) if *actual_token == expected => Ok(()),
        Some(actual_token) => Err(raise_error(Expected::Tok(expected), actual_token)),
        None => Err(ParseError(format!(
            "Expected {:?} but reached end of tokens",
            expected
        ))),
    }
}

fn expect_empty(tokens: &mut std::slice::Iter<Token>) -> Result<(), ParseError> {
    match tokens.next() {
        None => Ok(()),
        Some(bad_token) => Err(raise_error(
            Expected::Name("end of file".to_string()),
            bad_token,
        )),
    }
}

fn parse_id(tokens: &mut std::slice::Iter<Token>) -> String {
    match tokens.next() {
        Some(Token::Identifier(x)) => x.clone(),
        Some(other) => {
            let expected = Expected::Name("an identifier".to_string());
            let actual = other;
            panic!("{}", raise_error(expected, actual).0);
        }
        None => panic!("Expected an identifier but reached end of tokens"),
    }
}

fn parse_expression(tokens: &mut std::slice::Iter<Token>) -> Exp {
    match tokens.next() {
        Some(Token::Constant(c)) => Exp::Constant(*c),
        Some(other) => {
            let expected = Expected::Name("an expression".to_string());
            let actual = other;
            panic!("{}", raise_error(expected, actual).0);
        }
        None => panic!("Expected an expression but reached end of tokens"),
    }
}

fn parse_statement(tokens: &mut std::slice::Iter<Token>) -> Statement {
    expect(Token::KWReturn, tokens).unwrap();
    let exp = parse_expression(tokens);
    expect(Token::Semicolon, tokens).unwrap();
    Statement::Return(exp)
}

fn parse_function_definition(tokens: &mut std::slice::Iter<Token>) -> FunctionDefinition {
    expect(Token::KWInt, tokens).unwrap();
    let fun_name = parse_id(tokens);
    expect(Token::OpenParen, tokens).unwrap();
    expect(Token::KWVoid, tokens).unwrap();
    expect(Token::CloseParen, tokens).unwrap();
    expect(Token::OpenBrace, tokens).unwrap();
    let statement = parse_statement(tokens);
    expect(Token::CloseBrace, tokens).unwrap();
    FunctionDefinition {
        name: fun_name,
        body: statement,
    }
}

pub fn parse(tokens: &[Token]) -> Result<Ast, ParseError> {
    let mut token_stream = tokens.clone().iter();
    let fun_def = parse_function_definition(&mut token_stream);
    expect_empty(&mut token_stream)?;

    Ok(Ast::Program(fun_def))
}
