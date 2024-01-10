#[derive(Debug, PartialEq)]
pub enum Platform {
    OsX,
    Linux,
}

impl Platform {
    pub fn show_label(&self, name: &str) -> String {
        match &self {
            Platform::OsX => format!("_{}", name),
            Platform::Linux => name.to_string(),
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Stage {
    Lex,
    Parse,
    Codegen,
    Assembly,
    Executable,
}

#[derive(Debug, PartialEq, Clone)]
pub enum Token {
    // tokens with contents
    Identifier(String),
    Constant(i32),
    // Keywords
    KWInt,
    KWReturn,
    KWVoid,
    // punctuation
    OpenParen,
    CloseParen,
    OpenBrace,
    CloseBrace,
    Semicolon,
}

#[derive(Debug)]
pub enum Exp {
    Constant(i32),
}

#[derive(Debug)]
pub enum Statement {
    Return(Exp),
}

#[derive(Debug)]
pub struct FunctionDefinition {
    pub name: String,
    pub body: Statement,
}

#[derive(Debug)]
pub enum Ast {
    Program(FunctionDefinition),
}

#[derive(Debug)]
pub enum Operand {
    Imm(i32),
    Register,
}

#[derive(Debug)]
pub enum Instruction {
    Mov(Operand, Operand),
    Ret,
}

#[derive(Debug)]
pub struct AsmFunctionDefinition {
    pub name: String,
    pub instructions: Vec<Instruction>,
}

#[derive(Debug)]
pub enum AsmAst {
    Program(AsmFunctionDefinition),
}
