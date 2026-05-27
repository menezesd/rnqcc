use crate::types::*;
use std::io::{self, Write};

fn invalid_input<T>(message: impl Into<String>) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn reg_name(reg: &Reg, t: AsmType) -> io::Result<&'static str> {
    match (reg, t) {
        (Reg::AX, AsmType::Longword) => Ok("%eax"),
        (Reg::AX, AsmType::Quadword) => Ok("%rax"),
        (Reg::BX, AsmType::Longword) => Ok("%ebx"),
        (Reg::BX, AsmType::Quadword) => Ok("%rbx"),
        (Reg::CX, AsmType::Longword) => Ok("%ecx"),
        (Reg::CX, AsmType::Quadword) => Ok("%rcx"),
        (Reg::DX, AsmType::Longword) => Ok("%edx"),
        (Reg::DX, AsmType::Quadword) => Ok("%rdx"),
        (Reg::DI, AsmType::Longword) => Ok("%edi"),
        (Reg::DI, AsmType::Quadword) => Ok("%rdi"),
        (Reg::SI, AsmType::Longword) => Ok("%esi"),
        (Reg::SI, AsmType::Quadword) => Ok("%rsi"),
        (Reg::R8, AsmType::Longword) => Ok("%r8d"),
        (Reg::R8, AsmType::Quadword) => Ok("%r8"),
        (Reg::R9, AsmType::Longword) => Ok("%r9d"),
        (Reg::R9, AsmType::Quadword) => Ok("%r9"),
        (Reg::R10, AsmType::Longword) => Ok("%r10d"),
        (Reg::R10, AsmType::Quadword) => Ok("%r10"),
        (Reg::R11, AsmType::Longword) => Ok("%r11d"),
        (Reg::R11, AsmType::Quadword) => Ok("%r11"),
        (Reg::R12, AsmType::Longword) => Ok("%r12d"),
        (Reg::R12, AsmType::Quadword) => Ok("%r12"),
        (Reg::R13, AsmType::Longword) => Ok("%r13d"),
        (Reg::R13, AsmType::Quadword) => Ok("%r13"),
        (Reg::R14, AsmType::Longword) => Ok("%r14d"),
        (Reg::R14, AsmType::Quadword) => Ok("%r14"),
        (Reg::R15, AsmType::Longword) => Ok("%r15d"),
        (Reg::R15, AsmType::Quadword) => Ok("%r15"),
        (Reg::SP, AsmType::Longword) => Ok("%esp"),
        (Reg::SP, AsmType::Quadword) => Ok("%rsp"),
        (Reg::BP, AsmType::Longword) => Ok("%ebp"),
        (Reg::BP, AsmType::Quadword) => Ok("%rbp"),
        // Byte: use 8-bit register names
        (Reg::AX, AsmType::Byte) => Ok("%al"),
        (Reg::AX, AsmType::Word) => Ok("%ax"),
        (Reg::BX, AsmType::Byte) => Ok("%bl"),
        (Reg::BX, AsmType::Word) => Ok("%bx"),
        (Reg::CX, AsmType::Byte) => Ok("%cl"),
        (Reg::CX, AsmType::Word) => Ok("%cx"),
        (Reg::DX, AsmType::Byte) => Ok("%dl"),
        (Reg::DX, AsmType::Word) => Ok("%dx"),
        (Reg::DI, AsmType::Byte) => Ok("%dil"),
        (Reg::DI, AsmType::Word) => Ok("%di"),
        (Reg::SI, AsmType::Byte) => Ok("%sil"),
        (Reg::SI, AsmType::Word) => Ok("%si"),
        (Reg::R8, AsmType::Byte) => Ok("%r8b"),
        (Reg::R8, AsmType::Word) => Ok("%r8w"),
        (Reg::R9, AsmType::Byte) => Ok("%r9b"),
        (Reg::R9, AsmType::Word) => Ok("%r9w"),
        (Reg::R10, AsmType::Byte) => Ok("%r10b"),
        (Reg::R10, AsmType::Word) => Ok("%r10w"),
        (Reg::R11, AsmType::Byte) => Ok("%r11b"),
        (Reg::R11, AsmType::Word) => Ok("%r11w"),
        (Reg::R12, AsmType::Byte) => Ok("%r12b"),
        (Reg::R12, AsmType::Word) => Ok("%r12w"),
        (Reg::R13, AsmType::Byte) => Ok("%r13b"),
        (Reg::R13, AsmType::Word) => Ok("%r13w"),
        (Reg::R14, AsmType::Byte) => Ok("%r14b"),
        (Reg::R14, AsmType::Word) => Ok("%r14w"),
        (Reg::R15, AsmType::Byte) => Ok("%r15b"),
        (Reg::R15, AsmType::Word) => Ok("%r15w"),
        (Reg::SP, AsmType::Byte) => Ok("%spl"),
        (Reg::SP, AsmType::Word) => Ok("%sp"),
        (Reg::BP, AsmType::Byte) => Ok("%bpl"),
        (Reg::BP, AsmType::Word) => Ok("%bp"),
        (r, AsmType::Float | AsmType::Double) => {
            invalid_input(format!("Cannot use integer register {:?} for float", r))
        }
    }
}

fn xmm_name(reg: &XmmReg) -> &'static str {
    match reg {
        XmmReg::XMM0 => "%xmm0",
        XmmReg::XMM1 => "%xmm1",
        XmmReg::XMM2 => "%xmm2",
        XmmReg::XMM3 => "%xmm3",
        XmmReg::XMM4 => "%xmm4",
        XmmReg::XMM5 => "%xmm5",
        XmmReg::XMM6 => "%xmm6",
        XmmReg::XMM7 => "%xmm7",
        XmmReg::XMM8 => "%xmm8",
        XmmReg::XMM9 => "%xmm9",
        XmmReg::XMM10 => "%xmm10",
        XmmReg::XMM11 => "%xmm11",
        XmmReg::XMM12 => "%xmm12",
        XmmReg::XMM13 => "%xmm13",
        XmmReg::XMM14 => "%xmm14",
        XmmReg::XMM15 => "%xmm15",
    }
}

fn show_operand(op: &AsmOperand, t: AsmType, target: &Target) -> io::Result<String> {
    match op {
        AsmOperand::Imm(val) => Ok(format!("${}", val)),
        AsmOperand::Reg(reg) => Ok(reg_name(reg, t)?.to_string()),
        AsmOperand::Xmm(xmm) => Ok(xmm_name(xmm).to_string()),
        AsmOperand::Pseudo(name) => {
            invalid_input(format!("Pseudo-register '{}' not replaced", name))
        }
        AsmOperand::PseudoMem(name, _) => {
            invalid_input(format!("PseudoMem '{}' not replaced", name))
        }
        AsmOperand::Stack(offset) => Ok(format!("{}(%rbp)", offset)),
        AsmOperand::Data(name) => Ok(format!("{}(%rip)", target.show_label(name))),
        AsmOperand::TlsData(name, offset) => show_tls_operand(name, *offset, target),
        AsmOperand::Indexed(base, index, scale) => Ok(format!(
            "({}, {}, {})",
            reg_name(base, AsmType::Quadword)?,
            reg_name(index, AsmType::Quadword)?,
            scale
        )),
    }
}

fn show_operand_byte(op: &AsmOperand, target: &Target) -> io::Result<String> {
    match op {
        AsmOperand::Reg(reg) => Ok(reg_name(reg, AsmType::Byte)?.to_string()),
        AsmOperand::Stack(offset) => Ok(format!("{}(%rbp)", offset)),
        AsmOperand::Data(name) => Ok(format!("{}(%rip)", target.show_label(name))),
        AsmOperand::TlsData(name, offset) => show_tls_operand(name, *offset, target),
        other => invalid_input(format!("Cannot get byte-sized version of {:?}", other)),
    }
}

fn show_tls_operand(name: &str, offset: i32, target: &Target) -> io::Result<String> {
    match target.os {
        TargetOs::Linux => {
            let label = target.show_label(name);
            if offset == 0 {
                Ok(format!("%fs:{}@tpoff", label))
            } else if offset > 0 {
                Ok(format!("%fs:{}@tpoff+{}", label, offset))
            } else {
                Ok(format!("%fs:{}@tpoff{}", label, offset))
            }
        }
        TargetOs::MacOs => Ok(format!("{}(%rip)", target.show_label(name))),
    }
}

fn tls_offset_addr(offset: i32) -> String {
    if offset == 0 {
        "(%rax)".to_string()
    } else {
        format!("{}(%rax)", offset)
    }
}

fn emit_macho_tls_address_to_rax(
    w: &mut dyn Write,
    name: &str,
    offset: i32,
    target: &Target,
) -> io::Result<()> {
    writeln!(w, "\tmovq {}@TLVP(%rip), %rdi", target.show_label(name))?;
    writeln!(w, "\tcallq *(%rdi)")?;
    if offset != 0 {
        writeln!(w, "\taddq ${}, %rax", offset)?;
    }
    Ok(())
}

fn emit_macho_tls_load(
    w: &mut dyn Write,
    ty: AsmType,
    name: &str,
    offset: i32,
    dst: &AsmOperand,
    target: &Target,
) -> io::Result<()> {
    emit_macho_tls_address_to_rax(w, name, offset, target)?;
    writeln!(
        w,
        "\tmov{} {}, {}",
        suffix(ty),
        tls_offset_addr(0),
        show_operand(dst, ty, target)?
    )
}

fn emit_macho_tls_store(
    w: &mut dyn Write,
    ty: AsmType,
    src: &AsmOperand,
    name: &str,
    offset: i32,
    target: &Target,
) -> io::Result<()> {
    match ty {
        AsmType::Float | AsmType::Double => {
            if matches!(src, AsmOperand::Xmm(_)) {
                writeln!(w, "\tsubq $16, %rsp")?;
                writeln!(
                    w,
                    "\tmov{} {}, (%rsp)",
                    suffix(ty),
                    show_operand(src, ty, target)?
                )?;
                emit_macho_tls_address_to_rax(w, name, offset, target)?;
                writeln!(w, "\tmov{} (%rsp), %xmm15", suffix(ty))?;
                writeln!(w, "\tmov{} %xmm15, {}", suffix(ty), tls_offset_addr(0))?;
                writeln!(w, "\taddq $16, %rsp")
            } else {
                emit_macho_tls_address_to_rax(w, name, offset, target)?;
                writeln!(
                    w,
                    "\tmov{} {}, %xmm15",
                    suffix(ty),
                    show_operand(src, ty, target)?
                )?;
                writeln!(w, "\tmov{} %xmm15, {}", suffix(ty), tls_offset_addr(0))
            }
        }
        _ => {
            let staged_reg = reg_name(&Reg::R11, ty)?;
            let staged = match src {
                AsmOperand::Imm(_) => false,
                _ => {
                    writeln!(
                        w,
                        "\tmov{} {}, {}",
                        suffix(ty),
                        show_operand(src, ty, target)?,
                        staged_reg
                    )?;
                    true
                }
            };
            emit_macho_tls_address_to_rax(w, name, offset, target)?;
            let src = if staged {
                staged_reg.to_string()
            } else {
                show_operand(src, ty, target)?
            };
            writeln!(w, "\tmov{} {}, {}", suffix(ty), src, tls_offset_addr(0))
        }
    }
}

fn emit_tls_address(
    w: &mut dyn Write,
    name: &str,
    offset: i32,
    dst: &AsmOperand,
    target: &Target,
) -> std::io::Result<()> {
    match target.os {
        TargetOs::Linux => {
            let label = target.show_label(name);
            writeln!(w, "\tmovq %fs:0, %r11")?;
            let off = if offset == 0 {
                String::new()
            } else if offset > 0 {
                format!("+{}", offset)
            } else {
                offset.to_string()
            };
            let addr = format!("{}@tpoff{}(%r11)", label, off);
            match dst {
                AsmOperand::Reg(_) => writeln!(
                    w,
                    "\tleaq {}, {}",
                    addr,
                    show_operand(dst, AsmType::Quadword, target)?
                ),
                _ => {
                    writeln!(w, "\tleaq {}, %r10", addr)?;
                    writeln!(
                        w,
                        "\tmovq %r10, {}",
                        show_operand(dst, AsmType::Quadword, target)?
                    )
                }
            }
        }
        TargetOs::MacOs => {
            emit_macho_tls_address_to_rax(w, name, offset, target)?;
            if matches!(dst, AsmOperand::Reg(Reg::AX)) {
                Ok(())
            } else {
                writeln!(
                    w,
                    "\tmovq %rax, {}",
                    show_operand(dst, AsmType::Quadword, target)?
                )
            }
        }
    }
}

fn emit_atomic_rmw(
    w: &mut dyn Write,
    ty: AsmType,
    op: &AsmBinaryOp,
    return_old: bool,
    dst: &AsmOperand,
    target: &Target,
) -> std::io::Result<()> {
    let mnemonic = match op {
        AsmBinaryOp::Add => "add",
        AsmBinaryOp::Sub => "sub",
        AsmBinaryOp::And => "and",
        AsmBinaryOp::Nand => "and",
        AsmBinaryOp::Or => "or",
        AsmBinaryOp::Xor => "xor",
        _ => return invalid_input(format!("unsupported atomic rmw op: {:?}", op)),
    };
    let src = reg_name(&Reg::R10, ty)?;
    if return_old || matches!(op, AsmBinaryOp::Nand) {
        let old = reg_name(&Reg::AX, ty)?;
        let new = reg_name(&Reg::R12, ty)?;
        writeln!(w, "\tmov{} (%r11), {}", suffix(ty), old)?;
        writeln!(w, "1:")?;
        writeln!(w, "\tmov{} {}, {}", suffix(ty), old, new)?;
        writeln!(w, "\t{}{} {}, {}", mnemonic, suffix(ty), src, new)?;
        if matches!(op, AsmBinaryOp::Nand) {
            writeln!(w, "\tnot{} {}", suffix(ty), new)?;
        }
        writeln!(w, "\tlock cmpxchg{} {}, (%r11)", suffix(ty), new)?;
        writeln!(w, "\tjne 1b")?;
        let result = if return_old { old } else { new };
        return writeln!(
            w,
            "\tmov{} {}, {}",
            suffix(ty),
            result,
            show_operand(dst, ty, target)?
        );
    }
    writeln!(w, "\tlock {}{} {}, (%r11)", mnemonic, suffix(ty), src)?;
    if matches!(dst, AsmOperand::Reg(_)) {
        writeln!(
            w,
            "\tmov{} (%r11), {}",
            suffix(ty),
            show_operand(dst, ty, target)?
        )
    } else {
        writeln!(w, "\tmov{} (%r11), {}", suffix(ty), src)?;
        writeln!(
            w,
            "\tmov{} {}, {}",
            suffix(ty),
            src,
            show_operand(dst, ty, target)?
        )
    }
}

fn emit_atomic_exchange(
    w: &mut dyn Write,
    ty: AsmType,
    dst: &AsmOperand,
    target: &Target,
) -> std::io::Result<()> {
    let src = reg_name(&Reg::R10, ty)?;
    writeln!(w, "\txchg{} {}, (%r11)", suffix(ty), src)?;
    writeln!(
        w,
        "\tmov{} {}, {}",
        suffix(ty),
        src,
        show_operand(dst, ty, target)?
    )
}

fn emit_atomic_compare_exchange(
    w: &mut dyn Write,
    ty: AsmType,
    dst: &AsmOperand,
    target: &Target,
) -> std::io::Result<()> {
    let expected = reg_name(&Reg::AX, ty)?;
    let desired = reg_name(&Reg::R10, ty)?;
    writeln!(w, "\tmov{} (%r12), {}", suffix(ty), expected)?;
    writeln!(w, "\tlock cmpxchg{} {}, (%r11)", suffix(ty), desired)?;
    writeln!(w, "\tsete %r10b")?;
    writeln!(w, "\tje 1f")?;
    writeln!(w, "\tmov{} {}, (%r12)", suffix(ty), expected)?;
    writeln!(w, "1:")?;
    writeln!(
        w,
        "\tmovb %r10b, {}",
        show_operand(dst, AsmType::Byte, target)?
    )
}

fn emit_atomic_compare_swap(
    w: &mut dyn Write,
    ty: AsmType,
    return_old: bool,
    dst: &AsmOperand,
    target: &Target,
) -> std::io::Result<()> {
    let expected = reg_name(&Reg::AX, ty)?;
    let expected_src = reg_name(&Reg::R12, ty)?;
    let desired = reg_name(&Reg::R10, ty)?;
    writeln!(w, "\tmov{} {}, {}", suffix(ty), expected_src, expected)?;
    writeln!(w, "\tlock cmpxchg{} {}, (%r11)", suffix(ty), desired)?;
    if return_old {
        writeln!(
            w,
            "\tmov{} {}, {}",
            suffix(ty),
            expected,
            show_operand(dst, ty, target)?
        )
    } else {
        writeln!(w, "\tsete %r10b")?;
        writeln!(
            w,
            "\tmovb %r10b, {}",
            show_operand(dst, AsmType::Byte, target)?
        )
    }
}

fn show_operand_byte_or_imm(op: &AsmOperand, target: &Target) -> io::Result<String> {
    match op {
        AsmOperand::Imm(val) => Ok(format!("${}", val)),
        _ => show_operand_byte(op, target),
    }
}

fn show_operand_64(op: &AsmOperand, target: &Target) -> io::Result<String> {
    show_operand(op, AsmType::Quadword, target)
}

fn suffix(t: AsmType) -> &'static str {
    match t {
        AsmType::Byte => "b",
        AsmType::Word => "w",
        AsmType::Longword => "l",
        AsmType::Quadword => "q",
        AsmType::Float => "ss",
        AsmType::Double => "sd",
    }
}

fn show_cc(cc: &CondCode) -> &'static str {
    match cc {
        CondCode::E => "e",
        CondCode::NE => "ne",
        CondCode::L => "l",
        CondCode::LE => "le",
        CondCode::G => "g",
        CondCode::GE => "ge",
        CondCode::A => "a",
        CondCode::AE => "ae",
        CondCode::B => "b",
        CondCode::BE => "be",
    }
}

fn emit_instruction(w: &mut dyn Write, instr: &AsmInstr, platform: &Target) -> std::io::Result<()> {
    match instr {
        AsmInstr::Mov(t, src, dst) => {
            if platform.os == TargetOs::MacOs {
                if let AsmOperand::TlsData(name, offset) = src {
                    return emit_macho_tls_load(w, *t, name, *offset, dst, platform);
                }
                if let AsmOperand::TlsData(name, offset) = dst {
                    return emit_macho_tls_store(w, *t, src, name, *offset, platform);
                }
            }
            if matches!(*t, AsmType::Float | AsmType::Double) {
                writeln!(
                    w,
                    "\tmov{} {}, {}",
                    suffix(*t),
                    show_operand(src, *t, platform)?,
                    show_operand(dst, *t, platform)?
                )
            } else if *t == AsmType::Byte {
                writeln!(
                    w,
                    "\tmovb {}, {}",
                    show_operand_byte_or_imm(src, platform)?,
                    show_operand_byte(dst, platform)?
                )
            } else {
                // For 64-bit immediates that don't fit in 32-bit sign-extended,
                // if dst is a register, emit movabsq directly; otherwise use r10
                if *t == AsmType::Quadword {
                    if let AsmOperand::Imm(v) = src {
                        if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                            if matches!(dst, AsmOperand::Reg(_)) {
                                return writeln!(
                                    w,
                                    "\tmovq ${}, {}",
                                    v,
                                    show_operand(dst, *t, platform)?
                                );
                            }
                            writeln!(w, "\tmovq ${}, %r10", v)?;
                            return writeln!(
                                w,
                                "\tmovq %r10, {}",
                                show_operand(dst, *t, platform)?
                            );
                        }
                    }
                }
                writeln!(
                    w,
                    "\tmov{} {}, {}",
                    suffix(*t),
                    show_operand(src, *t, platform)?,
                    show_operand(dst, *t, platform)?
                )
            }
        }
        AsmInstr::AtomicRmw(ty, op, return_old, dst) => {
            emit_atomic_rmw(w, *ty, op, *return_old, dst, platform)
        }
        AsmInstr::AtomicExchange(ty, dst) => emit_atomic_exchange(w, *ty, dst, platform),
        AsmInstr::AtomicCompareExchange(ty, dst) => {
            emit_atomic_compare_exchange(w, *ty, dst, platform)
        }
        AsmInstr::AtomicCompareSwap(ty, return_old, dst) => {
            emit_atomic_compare_swap(w, *ty, *return_old, dst, platform)
        }
        AsmInstr::Movsx(src_t, dst_t, src, dst) => {
            let mnemonic = match (src_t, dst_t) {
                (AsmType::Byte, AsmType::Longword) => "movsbl",
                (AsmType::Byte, AsmType::Quadword) => "movsbq",
                (AsmType::Word, AsmType::Longword) => "movswl",
                (AsmType::Word, AsmType::Quadword) => "movswq",
                (AsmType::Longword, AsmType::Quadword) => "movslq",
                _ => "movslq", // fallback
            };
            let src_str = if *src_t == AsmType::Byte {
                show_operand_byte_or_imm(src, platform)?
            } else {
                show_operand(src, *src_t, platform)?
            };
            writeln!(
                w,
                "\t{} {}, {}",
                mnemonic,
                src_str,
                show_operand(dst, *dst_t, platform)?
            )
        }
        AsmInstr::MovZeroExtend(src_t, dst_t, src, dst) => {
            let mnemonic = match (src_t, dst_t) {
                (AsmType::Byte, AsmType::Longword) => "movzbl",
                (AsmType::Byte, AsmType::Quadword) => "movzbq",
                (AsmType::Word, AsmType::Longword) => "movzwl",
                (AsmType::Word, AsmType::Quadword) => "movzwq",
                _ => "movl", // Longword→Quadword: movl zero-extends automatically
            };
            if *src_t == AsmType::Byte {
                writeln!(
                    w,
                    "\t{} {}, {}",
                    mnemonic,
                    show_operand_byte_or_imm(src, platform)?,
                    show_operand(dst, *dst_t, platform)?
                )
            } else {
                match dst {
                    AsmOperand::Reg(reg) => {
                        writeln!(
                            w,
                            "\t{} {}, {}",
                            mnemonic,
                            show_operand(src, *src_t, platform)?,
                            reg_name(reg, AsmType::Longword)?
                        )
                    }
                    _ => {
                        writeln!(
                            w,
                            "\t{} {}, {}",
                            mnemonic,
                            show_operand(src, *src_t, platform)?,
                            show_operand(dst, *src_t, platform)?
                        )
                    }
                }
            }
        }
        AsmInstr::Unary(t, op, operand) => {
            let mnemonic = match op {
                AsmUnaryOp::Neg => "neg",
                AsmUnaryOp::Not => "not",
            };
            writeln!(
                w,
                "\t{}{} {}",
                mnemonic,
                suffix(*t),
                show_operand(operand, *t, platform)?
            )
        }
        AsmInstr::Binary(t, op, src, dst) => {
            if matches!(*t, AsmType::Float | AsmType::Double) {
                let suffix = suffix(*t);
                let mnemonic = match op {
                    AsmBinaryOp::Add => format!("add{}", suffix),
                    AsmBinaryOp::Sub => format!("sub{}", suffix),
                    AsmBinaryOp::Mul => format!("mul{}", suffix),
                    AsmBinaryOp::SDiv | AsmBinaryOp::UDiv => {
                        return invalid_input("AArch64 integer division op reached x86_64 emitter")
                    }
                    AsmBinaryOp::DivDouble => format!("div{}", suffix),
                    AsmBinaryOp::Xor => {
                        if *t == AsmType::Float {
                            "xorps".to_string()
                        } else {
                            "xorpd".to_string()
                        }
                    }
                    _ => return invalid_input(format!("Unsupported floating binary op: {:?}", op)),
                };
                return writeln!(
                    w,
                    "\t{} {}, {}",
                    mnemonic,
                    show_operand(src, *t, platform)?,
                    show_operand(dst, *t, platform)?
                );
            }
            let mnemonic = match op {
                AsmBinaryOp::Add => "add",
                AsmBinaryOp::Sub => "sub",
                AsmBinaryOp::Mul => "imul",
                AsmBinaryOp::SDiv | AsmBinaryOp::UDiv => {
                    return invalid_input("AArch64 integer division op reached x86_64 emitter")
                }
                AsmBinaryOp::DivDouble => {
                    return invalid_input("DivDouble should only be used with Double type")
                }
                AsmBinaryOp::And => "and",
                AsmBinaryOp::Nand => {
                    return invalid_input("Nand should only be used by atomic RMW")
                }
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
                        _ => return invalid_input("Shift amount must be %cl or immediate"),
                    };
                    writeln!(
                        w,
                        "\t{}{} {}, {}",
                        mnemonic,
                        suffix(*t),
                        shift_src,
                        show_operand(dst, *t, platform)?
                    )
                }
                _ => {
                    // For imulq with large 64-bit immediates, load into r10 first
                    if *t == AsmType::Quadword && matches!(op, AsmBinaryOp::Mul) {
                        if let AsmOperand::Imm(v) = src {
                            if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                                writeln!(w, "\tmovq ${}, %r10", v)?;
                                return writeln!(
                                    w,
                                    "\timulq %r10, {}",
                                    show_operand(dst, *t, platform)?
                                );
                            }
                        }
                    }
                    // For other binary ops with large 64-bit immediates
                    if *t == AsmType::Quadword {
                        if let AsmOperand::Imm(v) = src {
                            if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                                writeln!(w, "\tmovq ${}, %r10", v)?;
                                return writeln!(
                                    w,
                                    "\t{}{} %r10, {}",
                                    mnemonic,
                                    suffix(*t),
                                    show_operand(dst, *t, platform)?
                                );
                            }
                        }
                    }
                    writeln!(
                        w,
                        "\t{}{} {}, {}",
                        mnemonic,
                        suffix(*t),
                        show_operand(src, *t, platform)?,
                        show_operand(dst, *t, platform)?
                    )
                }
            }
        }
        AsmInstr::Idiv(t, operand) => {
            writeln!(
                w,
                "\tidiv{} {}",
                suffix(*t),
                show_operand(operand, *t, platform)?
            )
        }
        AsmInstr::Div(t, operand) => {
            writeln!(
                w,
                "\tdiv{} {}",
                suffix(*t),
                show_operand(operand, *t, platform)?
            )
        }
        AsmInstr::Cdq(t) => match t {
            AsmType::Longword => writeln!(w, "\tcdq"),
            AsmType::Quadword => writeln!(w, "\tcqo"),
            _ => invalid_input("cdq not used with byte/double"),
        },
        AsmInstr::Cmp(t, src, dst) => {
            if matches!(*t, AsmType::Float | AsmType::Double) {
                writeln!(
                    w,
                    "\tcomi{} {}, {}",
                    suffix(*t),
                    show_operand(src, *t, platform)?,
                    show_operand(dst, *t, platform)?
                )
            } else {
                // cmpq doesn't support 64-bit immediates
                if *t == AsmType::Quadword {
                    if let AsmOperand::Imm(v) = src {
                        if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                            writeln!(w, "\tmovq ${}, %r10", v)?;
                            return writeln!(
                                w,
                                "\tcmpq %r10, {}",
                                show_operand(dst, *t, platform)?
                            );
                        }
                    }
                }
                writeln!(
                    w,
                    "\tcmp{} {}, {}",
                    suffix(*t),
                    show_operand(src, *t, platform)?,
                    show_operand(dst, *t, platform)?
                )
            }
        }
        AsmInstr::Lea(src, dst) => {
            if let AsmOperand::TlsData(name, offset) = src {
                return emit_tls_address(w, name, *offset, dst, platform);
            }
            writeln!(
                w,
                "\tleaq {}, {}",
                show_operand(src, AsmType::Quadword, platform)?,
                show_operand(dst, AsmType::Quadword, platform)?
            )
        }
        AsmInstr::LoadIndirect(t, reg, dst) => {
            // mov (reg), dst
            let reg64 = reg_name(reg, AsmType::Quadword)?;
            if *t == AsmType::Double {
                writeln!(
                    w,
                    "\tmovsd ({}), {}",
                    reg64,
                    show_operand(dst, *t, platform)?
                )
            } else {
                writeln!(
                    w,
                    "\tmov{} ({}), {}",
                    suffix(*t),
                    reg64,
                    show_operand(dst, *t, platform)?
                )
            }
        }
        AsmInstr::StoreIndirect(t, src, reg) => {
            // mov src, (reg)
            let reg64 = reg_name(reg, AsmType::Quadword)?;
            if *t == AsmType::Double {
                writeln!(
                    w,
                    "\tmovsd {}, ({})",
                    show_operand(src, *t, platform)?,
                    reg64
                )
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
                writeln!(
                    w,
                    "\tmov{} {}, ({})",
                    suffix(*t),
                    show_operand(src, *t, platform)?,
                    reg64
                )
            }
        }
        AsmInstr::AArch64Rem(..) => {
            invalid_input("AArch64 remainder instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64AddPtr(..) => {
            invalid_input("AArch64 pointer-add instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64LoadAdjusted(..) => {
            invalid_input("AArch64 adjusted load instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64StoreOutgoingArg(..) => {
            invalid_input("AArch64 outgoing argument instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64SaveLink(..) | AsmInstr::AArch64RestoreLink(..) => {
            invalid_input("AArch64 link-register instruction reached x86_64 emitter")
        }
        AsmInstr::Cvtsi2sd(src_t, src, dst) => {
            writeln!(
                w,
                "\tcvtsi2sd{} {}, {}",
                if *src_t == AsmType::Quadword {
                    "q"
                } else {
                    "l"
                },
                show_operand(src, *src_t, platform)?,
                show_operand(dst, AsmType::Double, platform)?
            )
        }
        AsmInstr::Cvtsi2ss(src_t, src, dst) => {
            writeln!(
                w,
                "\tcvtsi2ss{} {}, {}",
                if *src_t == AsmType::Quadword {
                    "q"
                } else {
                    "l"
                },
                show_operand(src, *src_t, platform)?,
                show_operand(dst, AsmType::Float, platform)?
            )
        }
        AsmInstr::Cvttsd2si(dst_t, src, dst) => {
            writeln!(
                w,
                "\tcvttsd2si{} {}, {}",
                if *dst_t == AsmType::Quadword {
                    "q"
                } else {
                    "l"
                },
                show_operand(src, AsmType::Double, platform)?,
                show_operand(dst, *dst_t, platform)?
            )
        }
        AsmInstr::Cvttss2si(dst_t, src, dst) => {
            writeln!(
                w,
                "\tcvttss2si{} {}, {}",
                if *dst_t == AsmType::Quadword {
                    "q"
                } else {
                    "l"
                },
                show_operand(src, AsmType::Float, platform)?,
                show_operand(dst, *dst_t, platform)?
            )
        }
        AsmInstr::Cvtss2sd(src, dst) => writeln!(
            w,
            "\tcvtss2sd {}, {}",
            show_operand(src, AsmType::Float, platform)?,
            show_operand(dst, AsmType::Double, platform)?
        ),
        AsmInstr::Cvtsd2ss(src, dst) => writeln!(
            w,
            "\tcvtsd2ss {}, {}",
            show_operand(src, AsmType::Double, platform)?,
            show_operand(dst, AsmType::Float, platform)?
        ),
        AsmInstr::AArch64UIntToDouble(..) => {
            invalid_input("AArch64 unsigned-to-double instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64UIntToFloat(..) => {
            invalid_input("AArch64 unsigned-to-float instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64DoubleToUInt(..) => {
            invalid_input("AArch64 double-to-unsigned instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64FloatToUInt(..) => {
            invalid_input("AArch64 float-to-unsigned instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64FloatToDouble(..) => {
            invalid_input("AArch64 float-to-double instruction reached x86_64 emitter")
        }
        AsmInstr::AArch64DoubleToFloat(..) => {
            invalid_input("AArch64 double-to-float instruction reached x86_64 emitter")
        }
        AsmInstr::X86SetVarargsXmmCount(count) => {
            writeln!(w, "\tmovb ${}, %al", count)
        }
        AsmInstr::AtomicFence => writeln!(w, "\tmfence"),
        AsmInstr::Jmp(label) => writeln!(w, "\tjmp .L{}", label),
        AsmInstr::JmpCC(cc, label) => writeln!(w, "\tj{} .L{}", show_cc(cc), label),
        AsmInstr::SetCC(cc, operand) => {
            writeln!(
                w,
                "\tset{} {}",
                show_cc(cc),
                show_operand_byte(operand, platform)?
            )
        }
        AsmInstr::Label(label) => writeln!(w, ".L{}:", label),
        AsmInstr::Push(operand) => {
            // pushq doesn't support XMM registers
            if let AsmOperand::Xmm(xmm) = operand {
                writeln!(w, "\tsubq $8, %rsp")?;
                return writeln!(w, "\tmovsd {}, (%rsp)", xmm_name(xmm));
            }
            // pushq doesn't support 64-bit immediates
            if let AsmOperand::Imm(v) = operand {
                if *v > i32::MAX as i64 || *v < i32::MIN as i64 {
                    writeln!(w, "\tmovq ${}, %r10", v)?;
                    return writeln!(w, "\tpushq %r10");
                }
            }
            writeln!(w, "\tpushq {}", show_operand_64(operand, platform)?)
        }
        AsmInstr::Pop(reg) => {
            writeln!(w, "\tpopq {}", reg_name(reg, AsmType::Quadword)?)
        }
        AsmInstr::Call(name, _, _, indirect) => {
            if *indirect {
                // Indirect call through R10 (function pointer already loaded there)
                writeln!(w, "\tcall *%r10")
            } else {
                let label = platform.show_label(name);
                match platform.os {
                    TargetOs::MacOs => writeln!(w, "\tcall {}", label),
                    TargetOs::Linux => writeln!(w, "\tcall {}@PLT", label),
                }
            }
        }
        AsmInstr::Ret => {
            writeln!(w, "\tmovq %rbp, %rsp")?;
            writeln!(w, "\tpopq %rbp")?;
            writeln!(w, "\tret")
        }
        AsmInstr::Unreachable => {
            writeln!(w, "\tmovq %rbp, %rsp")?;
            writeln!(w, "\tpopq %rbp")?;
            writeln!(w, "\tud2")
        }
        AsmInstr::AllocateStack(size) => {
            if *size > 0 {
                writeln!(w, "\tsubq ${}, %rsp", size)
            } else {
                Ok(())
            }
        }
        AsmInstr::DeallocateStack(size) => {
            if *size > 0 {
                writeln!(w, "\taddq ${}, %rsp", size)
            } else {
                Ok(())
            }
        }
    }
}

fn emit_function(w: &mut dyn Write, func: &AsmFunction, platform: &Target) -> std::io::Result<()> {
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

fn static_init_size(init: &StaticInit) -> usize {
    match init {
        StaticInit::CharInit(_) | StaticInit::UCharInit(_) => 1,
        StaticInit::ShortInit(_) | StaticInit::UShortInit(_) => 2,
        StaticInit::IntInit(_) | StaticInit::UIntInit(_) | StaticInit::FloatInit(_) => 4,
        StaticInit::LongInit(_)
        | StaticInit::ULongInit(_)
        | StaticInit::DoubleInit(_)
        | StaticInit::PointerInit(_)
        | StaticInit::PointerInitOffset(_, _) => 8,
        StaticInit::ZeroInit(n) => *n,
        StaticInit::StringInit(s, null_terminated) => {
            c_string_byte_len(s) + usize::from(*null_terminated)
        }
    }
}

fn alignment_log2(alignment: usize) -> usize {
    alignment.next_power_of_two().trailing_zeros() as usize
}

fn emit_macho_tls_static_var(
    w: &mut dyn Write,
    sv: &AsmStaticVar,
    platform: &Target,
    all_zero: bool,
) -> std::io::Result<()> {
    let label = platform.show_label(&sv.name);
    let init_label = format!("{}$tlv$init", label);
    let size: usize = sv.init_values.iter().map(static_init_size).sum();

    if all_zero {
        writeln!(
            w,
            "\t.tbss {},{},{}",
            init_label,
            size,
            alignment_log2(sv.alignment)
        )?;
    } else {
        writeln!(w, "\t.section __DATA,__thread_data,thread_local_regular")?;
        writeln!(w, "\t.balign {}", sv.alignment)?;
        writeln!(w, "{}:", init_label)?;
        for init in &sv.init_values {
            emit_static_init(w, init, platform)?;
        }
    }

    writeln!(w, "\t.section __DATA,__thread_vars,thread_local_variables")?;
    if sv.global {
        writeln!(w, "\t.globl {}", label)?;
    }
    writeln!(w, "\t.balign 8")?;
    writeln!(w, "{}:", label)?;
    writeln!(w, "\t.quad __tlv_bootstrap")?;
    writeln!(w, "\t.quad 0")?;
    writeln!(w, "\t.quad {}", init_label)
}

fn emit_static_var(w: &mut dyn Write, sv: &AsmStaticVar, platform: &Target) -> std::io::Result<()> {
    let label = platform.show_label(&sv.name);
    let all_zero = sv
        .init_values
        .iter()
        .all(|v| matches!(v, StaticInit::ZeroInit(_)))
        && !sv.init_values.is_empty();

    if sv.thread_local {
        if platform.os == TargetOs::MacOs {
            return emit_macho_tls_static_var(w, sv, platform, all_zero);
        }
        match (platform.os, all_zero) {
            (TargetOs::Linux, true) => writeln!(w, "\t.section .tbss,\"awT\",@nobits")?,
            (TargetOs::Linux, false) => writeln!(w, "\t.section .tdata,\"awT\",@progbits")?,
            (TargetOs::MacOs, true) => writeln!(w, "\t.bss")?,
            (TargetOs::MacOs, false) => writeln!(w, "\t.data")?,
        }
    } else if all_zero {
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
    for b in c_string_bytes(s) {
        match b {
            b'\\' => out.push_str("\\\\"),
            b'"' => out.push_str("\\\""),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            0 => out.push_str("\\0"),
            // Use octal for control chars that assembler may not support
            b if (0x20..0x7f).contains(&b) => out.push(b as char),
            b => {
                out.push_str(&format!("\\{:03o}", b));
            }
        }
    }
    out
}

fn emit_static_init(
    w: &mut dyn Write,
    init: &StaticInit,
    platform: &Target,
) -> std::io::Result<()> {
    match init {
        StaticInit::CharInit(v) => writeln!(w, "\t.byte {}", *v as u8),
        StaticInit::UCharInit(v) => writeln!(w, "\t.byte {}", v),
        StaticInit::ShortInit(v) => writeln!(w, "\t.short {}", v),
        StaticInit::UShortInit(v) => writeln!(w, "\t.short {}", v),
        StaticInit::IntInit(v) => writeln!(w, "\t.long {}", v),
        StaticInit::LongInit(v) => writeln!(w, "\t.quad {}", v),
        StaticInit::UIntInit(v) => writeln!(w, "\t.long {}", v),
        StaticInit::ULongInit(v) => writeln!(w, "\t.quad {}", v),
        StaticInit::FloatInit(v) => writeln!(w, "\t.long {}", v.to_bits()),
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
        StaticInit::PointerInitOffset(label, offset) => {
            let sign = if *offset >= 0 { "+" } else { "" };
            writeln!(
                w,
                "\t.quad {}{}{}",
                platform.show_label(label),
                sign,
                offset
            )
        }
    }
}

fn emit_static_constant(
    w: &mut dyn Write,
    sc: &AsmStaticConstant,
    platform: &Target,
) -> std::io::Result<()> {
    let label = platform.show_label(&sc.name);
    // Constant strings go in read-only section
    match platform.os {
        TargetOs::MacOs => writeln!(w, "\t.section __TEXT,__cstring")?,
        TargetOs::Linux => writeln!(w, "\t.section .rodata")?,
    }
    if sc.alignment > 1 {
        writeln!(w, "\t.balign {}", sc.alignment)?;
    }
    writeln!(w, "{}:", label)?;
    emit_static_init(w, &sc.init, platform)?;
    Ok(())
}

fn emit_stack_note(w: &mut dyn Write, platform: &Target) -> std::io::Result<()> {
    match platform.os {
        TargetOs::Linux => writeln!(w, "\t.section .note.GNU-stack,\"\",@progbits"),
        TargetOs::MacOs => Ok(()),
    }
}

pub fn emit(assembly_file: &str, program: &AsmProgram, platform: &Target) -> std::io::Result<()> {
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
