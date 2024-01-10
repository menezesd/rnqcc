use crate::types::*;

fn show_operand(operand: &Operand) -> String {
    match operand {
        Operand::Register => "%eax".to_string(),
        Operand::Imm(i) => format!("${}", i),
    }
}

fn emit_instruction(chan: &mut dyn std::io::Write, instr: &Instruction) -> std::io::Result<()> {
    match instr {
        Instruction::Mov(src, dst) => {
            writeln!(chan, "\tmovl {}, {}", show_operand(src), show_operand(dst))
        }
        Instruction::Ret => writeln!(chan, "\tret"),
    }
}

fn emit_function(
    chan: &mut dyn std::io::Write,
    func: &AsmFunctionDefinition,
    settings: &Platform,
) -> std::io::Result<()> {
    let label = settings.show_label(&func.name);
    writeln!(chan, ".globl {}", label)?;
    writeln!(chan, "{}:", label)?;

    for instruction in &func.instructions {
        emit_instruction(chan, instruction)?;
    }

    Ok(())
}

fn emit_stack_note(chan: &mut dyn std::io::Write, settings: &Platform) -> std::io::Result<()> {
    match &settings {
        Platform::Linux => writeln!(chan, "\t.section .note.GNU-stack,\"\",@progbits"),
        Platform::OsX => Ok(()),
    }
}

pub fn emit(assembly_file: &str, program: &AsmAst, settings: &Platform) -> std::io::Result<()> {
    let mut output_channel = std::fs::File::create(assembly_file)?;
    let AsmAst::Program(function_def) = program;
    emit_function(&mut output_channel, function_def, settings)?;
    emit_stack_note(&mut output_channel, settings)?;
    Ok(())
}
