//! TACKY (Three-Address Code Kit for Yacc) IR generation
//!
//! This module converts the resolved AST into TACKY IR, a target-independent
//! three-address code representation suitable for optimization and code generation.

pub mod generator;

use crate::diagnostic::Warning;
use crate::types::*;

pub type TackyResult<T> = Result<T, String>;

pub struct TackyOutput {
    pub program: TackyProgram,
    pub warnings: Vec<Warning>,
}

/// Generate TACKY IR with default options (host target, no instrumentation)
pub fn generate(program: Program) -> TackyResult<TackyProgram> {
    generate_with_options(program, Target::host(), false)
}

/// Generate TACKY IR with custom options
pub fn generate_with_options(
    program: Program,
    target: Target,
    instrument_functions: bool,
) -> TackyResult<TackyProgram> {
    generate_with_target_options(program, target, instrument_functions, false)
}

/// Generate TACKY IR with target options
pub fn generate_with_target_options(
    program: Program,
    target: Target,
    instrument_functions: bool,
    permissive: bool,
) -> TackyResult<TackyProgram> {
    let output = generator::TackyGen::generate_with_options(
        program,
        target,
        instrument_functions,
        permissive,
    )?;
    Ok(output.program)
}

/// Generate TACKY IR with target options and warnings
pub fn generate_with_target_options_and_warnings(
    program: Program,
    target: Target,
    instrument_functions: bool,
    permissive: bool,
) -> TackyResult<TackyOutput> {
    generator::TackyGen::generate_with_options(program, target, instrument_functions, permissive)
}
