use crate::types::*;
use std::io::Write;

fn reg_name(reg: &Reg, t: AsmType) -> &'static str {
    match (reg, t) {
        (Reg::AX, AsmType::Longword) => "%eax",
        (Reg::AX, AsmType::Quadword) => "%rax",
        (Reg::CX, AsmType::Longword) => "%ecx",
        (Reg::CX, AsmType::Quadword) => "%rcx",
        (Reg::DX, AsmType::Longword) => "%edx",
        (Reg::DX, AsmType::Quadword) => "%rdx",
        (Reg::DI, AsmType::Longword) => "%edi",
        (Reg::DI, AsmType::Quadword) => "%rdi",
        (Reg::SI, AsmType::Longword) => "%esi",
        (Reg::SI, AsmType::Quadword) => "%rsi",
        (Reg::R8, AsmType::Longword) => "%r8d",
        (Reg::R8, AsmType::Quadword) => "%r8",
        (Reg::R9, AsmType::Longword) => "%r9d",
        (Reg::R9, AsmType::Quadword) => "%r9",
        (Reg::R10, AsmType::Longword) => "%r10d",
        (Reg::R10, AsmType::Quadword) => "%r10",
        (Reg::R11, AsmType::Longword) => "%r11d",
        (Reg::R11, AsmType::Quadword) => "%r11",
        // Byte: use 8-bit register names
        (Reg::AX, AsmType::Byte) => "%al",
        (Reg::CX, AsmType::Byte) => "%cl",
        (Reg::DX, AsmType::Byte) => "%dl",
        (Reg::DI, AsmType::Byte) => "%dil",
        (Reg::SI, AsmType::Byte) => "%sil",
        (Reg::R8, AsmType::Byte) => "%r8b",
        (Reg::R9, AsmType::Byte) => "%r9b",
        (Reg::R10, AsmType::Byte) => "%r10b",
        (Reg::R11, AsmType::Byte) => "%r11b",
        (r, AsmType::Double) => panic!("Cannot use integer register {:?} for double", r),
    }
}

fn reg_name_8(reg: &Reg) -> &'static str {
    match reg {
        Reg::AX => "%al",
        Reg::CX => "%cl",
        Reg::DX => "%dl",
        Reg::DI => "%dil",
        Reg::SI => "%sil",
        Reg::R8 => "%r8b",
        Reg::R9 => "%r9b",
        Reg::R10 => "%r10b",
        Reg::R11 => "%r11b",
    }
}

fn xmm_name(reg: &XmmReg) -> &'static str {
    match reg {
        XmmReg::XMM0 => "%xmm0", XmmReg::XMM1 => "%xmm1",
        XmmReg::XMM2 => "%xmm2", XmmReg::XMM3 => "%xmm3",
        XmmReg::XMM4 => "%xmm4", XmmReg::XMM5 => "%xmm5",
        XmmReg::XMM6 => "%xmm6", XmmReg::XMM7 => "%xmm7",
        XmmReg::XMM14 => "%xmm14", XmmReg::XMM15 => "%xmm15",
    }
}

fn show_operand(op: &AsmOperand, t: AsmType, platform: &Platform) -> String {
    match op {
        AsmOperand::Imm(val) => format!("${}", val),
        AsmOperand::Reg(reg) => reg_name(reg, t).to_string(),
        AsmOperand::Xmm(xmm) => xmm_name(xmm).to_string(),
        AsmOperand::Pseudo(name) => panic!("Pseudo-register '{}' not replaced", name),
        AsmOperand::PseudoMem(name, _) => panic!("PseudoMem '{}' not replaced", name),
        AsmOperand::Stack(offset) => format!("{}(%rbp)", offset),
        AsmOperand::Data(name) => format!("{}(%rip)", platform.show_label(name)),
        AsmOperand::Indexed(base, index, scale) => {
            format!("({}, {}, {})",
                reg_name(base, AsmType::Quadword),
                reg_name(index, AsmType::Quadword),
                scale)
        }
    }
}

fn show_operand_byte(op: &AsmOperand, platform: &Platform) -> String {
    match op {
        AsmOperand::Reg(reg) => reg_name_8(reg).to_string(),
        AsmOperand::Stack(offset) => format!("{}(%rbp)", offset),
        AsmOperand::Data(name) => format!("{}(%rip)", platform.show_label(name)),
        other => panic!("Cannot get byte-sized version of {:?}", other),
    }
}

fn show_operand_byte_or_imm(op: &AsmOperand, platform: &Platform) -> String {
    match op {
        AsmOperand::Imm(val) => format!("${}", val),
        _ => show_operand_byte(op, platform),
    }
}

fn show_operand_64(op: &AsmOperand, platform: &Platform) -> String {
    show_operand(op, AsmType::Quadword, platform)
}

fn suffix(t: AsmType) -> &'static str {
    match t { AsmType::Byte => "b", AsmType::Longword => "l", AsmType::Quadword => "q", AsmType::Double => "sd" }
}

fn show_cc(cc: &CondCode) -> &'static str {
    match cc {
        CondCode::E => "e", CondCode::NE => "ne",
        CondCode::L => "l", CondCode::LE => "le",
        CondCode::G => "g", CondCode::GE => "ge",
        CondCode::A => "a", CondCode::AE => "ae",
        CondCode::B => "b", CondCode::BE => "be",
    }
}

fn emit_instruction(w: &mut dyn Write, instr: &AsmInstr, platform: &Platform) -> std::io::Result<()> {
    match instr {
        AsmInstr::Mov(t, src, dst) => {
            if *t == AsmType::Double {
                writeln!(w, "\tmovsd {}, {}", show_operand(src, *t, platform), show_operand(dst, *t, platform))
            } else if *t == AsmType::Byte {
                writeln!(w, "\tmovb {}, {}", show_operand_byte_or_imm(src, platform), show_operand_byte(dst, platform))
            } else {
                // For 64-bit immediates that don't fit in 32-bit sign-extended,
                // if dst is a register, emit movabsq directly; otherwise use r10
                if *t == AsmType::Quadword {
                    if let AsmOperand::Imm(v) = src {
                        if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                            if matches!(dst, AsmOperand::Reg(_)) {
                                return writeln!(w, "\tmovq ${}, {}", v, show_operand(dst, *t, platform));
                            }
                            writeln!(w, "\tmovq ${}, %r10", v)?;
                            return writeln!(w, "\tmovq %r10, {}", show_operand(dst, *t, platform));
                        }
                    }
                }
                writeln!(w, "\tmov{} {}, {}", suffix(*t), show_operand(src, *t, platform), show_operand(dst, *t, platform))
            }
        }
        AsmInstr::Movsx(src_t, dst_t, src, dst) => {
            let mnemonic = match (src_t, dst_t) {
                (AsmType::Byte, AsmType::Longword) => "movsbl",
                (AsmType::Byte, AsmType::Quadword) => "movsbq",
                (AsmType::Longword, AsmType::Quadword) => "movslq",
                _ => "movslq", // fallback
            };
            let src_str = if *src_t == AsmType::Byte {
                show_operand_byte_or_imm(src, platform)
            } else {
                show_operand(src, *src_t, platform)
            };
            writeln!(w, "\t{} {}, {}", mnemonic, src_str, show_operand(dst, *dst_t, platform))
        }
        AsmInstr::MovZeroExtend(src_t, dst_t, src, dst) => {
            let mnemonic = match (src_t, dst_t) {
                (AsmType::Byte, AsmType::Longword) => "movzbl",
                (AsmType::Byte, AsmType::Quadword) => "movzbq",
                _ => "movl", // Longword→Quadword: movl zero-extends automatically
            };
            if *src_t == AsmType::Byte {
                writeln!(w, "\t{} {}, {}", mnemonic, show_operand_byte_or_imm(src, platform), show_operand(dst, *dst_t, platform))
            } else {
                match dst {
                    AsmOperand::Reg(reg) => {
                        writeln!(w, "\t{} {}, {}", mnemonic, show_operand(src, *src_t, platform), reg_name(reg, AsmType::Longword))
                    }
                    _ => {
                        writeln!(w, "\t{} {}, {}", mnemonic, show_operand(src, *src_t, platform), show_operand(dst, *src_t, platform))
                    }
                }
            }
        }
        AsmInstr::Unary(t, op, operand) => {
            let mnemonic = match op {
                AsmUnaryOp::Neg => "neg",
                AsmUnaryOp::Not => "not",
            };
            writeln!(w, "\t{}{} {}", mnemonic, suffix(*t), show_operand(operand, *t, platform))
        }
        AsmInstr::Binary(t, op, src, dst) => {
            if *t == AsmType::Double {
                let mnemonic = match op {
                    AsmBinaryOp::Add => "addsd",
                    AsmBinaryOp::Sub => "subsd",
                    AsmBinaryOp::Mul => "mulsd",
                    AsmBinaryOp::DivDouble => "divsd",
                    AsmBinaryOp::Xor => "xorpd",
                    _ => panic!("Unsupported double binary op: {:?}", op),
                };
                return writeln!(w, "\t{} {}, {}", mnemonic, show_operand(src, *t, platform), show_operand(dst, *t, platform));
            }
            let mnemonic = match op {
                AsmBinaryOp::Add => "add",
                AsmBinaryOp::Sub => "sub",
                AsmBinaryOp::Mul => "imul",
                AsmBinaryOp::DivDouble => unreachable!("DivDouble should only be used with Double type"),
                AsmBinaryOp::And => "and",
                AsmBinaryOp::Or => "or",
                AsmBinaryOp::Xor => "xor",
                AsmBinaryOp::Sal => "sal",
                AsmBinaryOp::Sar => "sar",
                AsmBinaryOp::Shr => "shr",
            };
            match op {
                AsmBinaryOp::Sal | AsmBinaryOp::Sar | AsmBinaryOp::Shr => {
                    let shift_src = match src {
                        AsmOperand::Reg(Reg::CX) => "%cl".to_string(),
                        AsmOperand::Imm(val) => format!("${}", val),
                        _ => panic!("Shift amount must be %%cl or immediate"),
                    };
                    writeln!(w, "\t{}{} {}, {}", mnemonic, suffix(*t), shift_src, show_operand(dst, *t, platform))
                }
                _ => {
                    // For imulq with large 64-bit immediates, load into r10 first
                    if *t == AsmType::Quadword && matches!(op, AsmBinaryOp::Mul) {
                        if let AsmOperand::Imm(v) = src {
                            if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                                writeln!(w, "\tmovq ${}, %r10", v)?;
                                return writeln!(w, "\timulq %r10, {}", show_operand(dst, *t, platform));
                            }
                        }
                    }
                    // For other binary ops with large 64-bit immediates
                    if *t == AsmType::Quadword {
                        if let AsmOperand::Imm(v) = src {
                            if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                                writeln!(w, "\tmovq ${}, %r10", v)?;
                                return writeln!(w, "\t{}{} %r10, {}", mnemonic, suffix(*t), show_operand(dst, *t, platform));
                            }
                        }
                    }
                    writeln!(w, "\t{}{} {}, {}", mnemonic, suffix(*t), show_operand(src, *t, platform), show_operand(dst, *t, platform))
                }
            }
        }
        AsmInstr::Idiv(t, operand) => {
            writeln!(w, "\tidiv{} {}", suffix(*t), show_operand(operand, *t, platform))
        }
        AsmInstr::Div(t, operand) => {
            writeln!(w, "\tdiv{} {}", suffix(*t), show_operand(operand, *t, platform))
        }
        AsmInstr::Cdq(t) => {
            match t {
                AsmType::Longword => writeln!(w, "\tcdq"),
                AsmType::Quadword => writeln!(w, "\tcqo"),
                _ => unreachable!("cdq not used with byte/double"),
            }
        }
        AsmInstr::Cmp(t, src, dst) => {
            if *t == AsmType::Double {
                writeln!(w, "\tcomisd {}, {}", show_operand(src, *t, platform), show_operand(dst, *t, platform))
            } else {
                // cmpq doesn't support 64-bit immediates
                if *t == AsmType::Quadword {
                    if let AsmOperand::Imm(v) = src {
                        if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                            writeln!(w, "\tmovq ${}, %r10", v)?;
                            return writeln!(w, "\tcmpq %r10, {}", show_operand(dst, *t, platform));
                        }
                    }
                }
                writeln!(w, "\tcmp{} {}, {}", suffix(*t), show_operand(src, *t, platform), show_operand(dst, *t, platform))
            }
        }
        AsmInstr::Lea(src, dst) => {
            writeln!(w, "\tleaq {}, {}", show_operand(src, AsmType::Quadword, platform), show_operand(dst, AsmType::Quadword, platform))
        }
        AsmInstr::LoadIndirect(t, reg, dst) => {
            // mov (reg), dst
            let reg64 = reg_name(reg, AsmType::Quadword);
            if *t == AsmType::Double {
                writeln!(w, "\tmovsd ({}), {}", reg64, show_operand(dst, *t, platform))
            } else {
                writeln!(w, "\tmov{} ({}), {}", suffix(*t), reg64, show_operand(dst, *t, platform))
            }
        }
        AsmInstr::StoreIndirect(t, src, reg) => {
            // mov src, (reg)
            let reg64 = reg_name(reg, AsmType::Quadword);
            if *t == AsmType::Double {
                writeln!(w, "\tmovsd {}, ({})", show_operand(src, *t, platform), reg64)
            } else {
                // Handle 64-bit immediates that don't fit in 32-bit sign-extended
                if *t == AsmType::Quadword {
                    if let AsmOperand::Imm(v) = src {
                        if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                            writeln!(w, "\tmovq ${}, %r10", v)?;
                            return writeln!(w, "\tmovq %r10, ({})", reg64);
                        }
                    }
                }
                writeln!(w, "\tmov{} {}, ({})", suffix(*t), show_operand(src, *t, platform), reg64)
            }
        }
        AsmInstr::Cvtsi2sd(src_t, src, dst) => {
            writeln!(w, "\tcvtsi2sd{} {}, {}",
                if *src_t == AsmType::Quadword { "q" } else { "l" },
                show_operand(src, *src_t, platform),
                show_operand(dst, AsmType::Double, platform))
        }
        AsmInstr::Cvttsd2si(dst_t, src, dst) => {
            writeln!(w, "\tcvttsd2si{} {}, {}",
                if *dst_t == AsmType::Quadword { "q" } else { "l" },
                show_operand(src, AsmType::Double, platform),
                show_operand(dst, *dst_t, platform))
        }
        AsmInstr::Jmp(label) => writeln!(w, "\tjmp .L{}", label),
        AsmInstr::JmpCC(cc, label) => writeln!(w, "\tj{} .L{}", show_cc(cc), label),
        AsmInstr::SetCC(cc, operand) => {
            writeln!(w, "\tset{} {}", show_cc(cc), show_operand_byte(operand, platform))
        }
        AsmInstr::Label(label) => writeln!(w, ".L{}:", label),
        AsmInstr::Push(operand) => {
            // pushq doesn't support 64-bit immediates
            if let AsmOperand::Imm(v) = operand {
                if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                    writeln!(w, "\tmovq ${}, %r10", v)?;
                    return writeln!(w, "\tpushq %r10");
                }
            }
            writeln!(w, "\tpushq {}", show_operand_64(operand, platform))
        }
        AsmInstr::Call(name) => {
            let label = platform.show_label(name);
            match platform {
                Platform::OsX => writeln!(w, "\tcall {}", label),
                Platform::Linux => writeln!(w, "\tcall {}@PLT", label),
            }
        }
        AsmInstr::Ret => {
            writeln!(w, "\tmovq %rbp, %rsp")?;
            writeln!(w, "\tpopq %rbp")?;
            writeln!(w, "\tret")
        }
        AsmInstr::AllocateStack(size) => {
            if *size > 0 { writeln!(w, "\tsubq ${}, %rsp", size) } else { Ok(()) }
        }
        AsmInstr::DeallocateStack(size) => {
            if *size > 0 { writeln!(w, "\taddq ${}, %rsp", size) } else { Ok(()) }
        }
    }
}

fn emit_function(w: &mut dyn Write, func: &AsmFunction, platform: &Platform) -> std::io::Result<()> {
    let label = platform.show_label(&func.name);
    writeln!(w, "\t.text")?;
    if func.global {
        writeln!(w, "\t.globl {}", label)?;
    }
    writeln!(w, "{}:", label)?;
    let mut iter = func.instructions.iter();
    if let Some(AsmInstr::Push(AsmOperand::Reg(Reg::AX))) = iter.next() {
        writeln!(w, "\tpushq %rbp")?;
        writeln!(w, "\tmovq %rsp, %rbp")?;
    }
    for instr in iter {
        emit_instruction(w, instr, platform)?;
    }
    Ok(())
}

fn emit_static_var(w: &mut dyn Write, sv: &AsmStaticVar, platform: &Platform) -> std::io::Result<()> {
    let label = platform.show_label(&sv.name);
    let all_zero = sv.init_values.iter().all(|v| matches!(v, StaticInit::ZeroInit(_)))
        && !sv.init_values.is_empty();

    if all_zero {
        writeln!(w, "\t.bss")?;
    } else {
        writeln!(w, "\t.data")?;
    }
    if sv.global {
        writeln!(w, "\t.globl {}", label)?;
    }
    writeln!(w, "\t.balign {}", sv.alignment)?;
    writeln!(w, "{}:", label)?;

    for init in &sv.init_values {
        emit_static_init(w, init, platform)?;
    }
    Ok(())
}

fn escape_string_for_asm(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0 => out.push_str("\\0"),
            // Use octal for control chars that assembler may not support
            b if b >= 0x20 && b < 0x7f => out.push(b as char),
            b => { out.push_str(&format!("\\{:03o}", b)); }
        }
    }
    out
}

fn emit_static_init(w: &mut dyn Write, init: &StaticInit, platform: &Platform) -> std::io::Result<()> {
    match init {
        StaticInit::CharInit(v) => writeln!(w, "\t.byte {}", *v as u8),
        StaticInit::UCharInit(v) => writeln!(w, "\t.byte {}", v),
        StaticInit::IntInit(v) => writeln!(w, "\t.long {}", v),
        StaticInit::LongInit(v) => writeln!(w, "\t.quad {}", v),
        StaticInit::UIntInit(v) => writeln!(w, "\t.long {}", v),
        StaticInit::ULongInit(v) => writeln!(w, "\t.quad {}", v),
        StaticInit::DoubleInit(v) => writeln!(w, "\t.quad {}", v.to_bits()),
        StaticInit::ZeroInit(n) => writeln!(w, "\t.zero {}", n),
        StaticInit::StringInit(s, null_terminated) => {
            let escaped = escape_string_for_asm(s);
            if *null_terminated {
                writeln!(w, "\t.asciz \"{}\"", escaped)
            } else {
                writeln!(w, "\t.ascii \"{}\"", escaped)
            }
        }
        StaticInit::PointerInit(label) => {
            writeln!(w, "\t.quad {}", platform.show_label(label))
        }
    }
}

fn emit_static_constant(w: &mut dyn Write, sc: &AsmStaticConstant, platform: &Platform) -> std::io::Result<()> {
    let label = platform.show_label(&sc.name);
    // Constant strings go in read-only section
    match platform {
        Platform::OsX => writeln!(w, "\t.section __TEXT,__cstring")?,
        Platform::Linux => writeln!(w, "\t.section .rodata")?,
    }
    if sc.alignment > 1 {
        writeln!(w, "\t.balign {}", sc.alignment)?;
    }
    writeln!(w, "{}:", label)?;
    emit_static_init(w, &sc.init, platform)?;
    Ok(())
}

fn emit_stack_note(w: &mut dyn Write, platform: &Platform) -> std::io::Result<()> {
    match platform {
        Platform::Linux => writeln!(w, "\t.section .note.GNU-stack,\"\",@progbits"),
        Platform::OsX => Ok(()),
    }
}

pub fn emit(assembly_file: &str, program: &AsmProgram, platform: &Platform) -> std::io::Result<()> {
    let mut w = std::fs::File::create(assembly_file)?;
    for tl in &program.top_level {
        match tl {
            AsmTopLevel::Function(func) => emit_function(&mut w, func, platform)?,
            AsmTopLevel::StaticVar(sv) => emit_static_var(&mut w, sv, platform)?,
            AsmTopLevel::StaticConstant(sc) => emit_static_constant(&mut w, sc, platform)?,
        }
    }
    emit_stack_note(&mut w, platform)?;
    Ok(())
}
