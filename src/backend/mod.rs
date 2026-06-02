pub mod aarch64;
pub mod x86_64;

use crate::types::*;

pub fn codegen(
    program: &TackyProgram,
    target: &Target,
    no_coalescing: bool,
) -> Result<AsmProgram, String> {
    match target.arch {
        Arch::X86_64 => x86_64::codegen::gen(program, target, no_coalescing),
        Arch::AArch64 => aarch64::codegen::gen(program, target),
    }
}

pub fn emit(assembly_file: &str, program: &AsmProgram, target: &Target) -> Result<(), String> {
    match target.arch {
        Arch::X86_64 => x86_64::emit::emit(assembly_file, program, target)
            .map_err(|err| format!("could not write {}: {}", assembly_file, err)),
        Arch::AArch64 => aarch64::emit::emit(assembly_file, program, target)
            .map_err(|err| format!("could not write {}: {}", assembly_file, err)),
    }
}
