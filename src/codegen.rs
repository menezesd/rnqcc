use crate::types::*;

fn convert_exp(exp: Exp) -> Operand {
    match exp {
        Exp::Constant(i) => Operand::Imm(i),
    }
}

fn convert_statement(stmt: Statement) -> Vec<Instruction> {
    let Statement::Return(exp) = stmt;
    let v = convert_exp(exp);
    vec![Instruction::Mov(v, Operand::Register), Instruction::Ret]
}

fn convert_function(fun: FunctionDefinition) -> AsmFunctionDefinition {
    let FunctionDefinition { name, body } = fun;
    let instructions = convert_statement(body);
    AsmFunctionDefinition { name, instructions }
}

pub fn gen(program: Ast) -> AsmAst {
    let Ast::Program(fn_def) = program;
    let function = convert_function(fn_def);
    AsmAst::Program(function)
}
