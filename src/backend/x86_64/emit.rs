use crate::types::*;
use std::io::{self, Write};

fn invalid_input<T>(message: impl Into<String>) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn fits_sign_extended_i32(value: i64) -> bool {
    i32::try_from(value).is_ok()
}

fn static_label_name(platform: &Target, label: &str) -> String {
    if label.starts_with("label.") {
        format!(".L{}", label)
    } else {
        platform.show_symbol(label)
    }
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
        (_, AsmType::Octword) => invalid_input("x86-64 emitter needs 128-bit integer lowering"),
        (_, AsmType::LongDouble) => invalid_input("x86-64 long double uses the x87 stack"),
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
        AsmOperand::StackArg(offset) => Ok(format!("{}(%rsp)", offset)),
        AsmOperand::Data(name) => Ok(format!("{}(%rip)", target.show_data_label_expr(name))),
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
        AsmOperand::StackArg(offset) => Ok(format!("{}(%rsp)", offset)),
        AsmOperand::Data(name) => Ok(format!("{}(%rip)", target.show_data_label_expr(name))),
        AsmOperand::TlsData(name, offset) => show_tls_operand(name, *offset, target),
        other => invalid_input(format!("Cannot get byte-sized version of {:?}", other)),
    }
}

fn show_tls_operand(name: &str, offset: i32, target: &Target) -> io::Result<String> {
    match target.os {
        TargetOs::Linux => {
            let label = target.show_symbol(name);
            if offset == 0 {
                Ok(format!("%fs:{}@tpoff", label))
            } else {
                Ok(format!(
                    "%fs:{}@tpoff{}",
                    label,
                    assembly_offset_suffix(i64::from(offset))
                ))
            }
        }
        TargetOs::MacOs => Ok(format!("{}(%rip)", target.show_symbol(name))),
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
    writeln!(w, "\tmovq {}@TLVP(%rip), %rdi", target.show_symbol(name))?;
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
            let label = target.show_symbol(name);
            writeln!(w, "\tmovq %fs:0, %r11")?;
            let off = if offset == 0 {
                String::new()
            } else {
                assembly_offset_suffix(i64::from(offset))
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
        AsmType::Octword => "q",
        AsmType::Float => "ss",
        AsmType::Double => "sd",
        AsmType::LongDouble => "t",
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
        CondCode::P => "p",
        CondCode::NP => "np",
        CondCode::S => "s",
        CondCode::NS => "ns",
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
            if src == dst && *t != AsmType::Longword {
                return Ok(());
            }
            if matches!(*t, AsmType::Float | AsmType::Double) {
                writeln!(
                    w,
                    "\tmov{} {}, {}",
                    suffix(*t),
                    show_operand(src, *t, platform)?,
                    show_operand(dst, *t, platform)?
                )
            } else if *t == AsmType::LongDouble {
                writeln!(
                    w,
                    "\tmovups {}, {}",
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
                        if !fits_sign_extended_i32(*v) {
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
        AsmInstr::X87Load(t, src) => {
            let mnemonic = match t {
                AsmType::Word => "filds",
                AsmType::Longword => "fildl",
                AsmType::Quadword => "fildll",
                AsmType::Float => "flds",
                AsmType::Double => "fldl",
                AsmType::LongDouble => "fldt",
                other => {
                    return invalid_input(format!(
                        "unsupported x87 load type in x86-64 emitter: {:?}",
                        other
                    ))
                }
            };
            if matches!(
                src,
                AsmOperand::Reg(_) | AsmOperand::Xmm(_) | AsmOperand::Imm(_)
            ) {
                writeln!(w, "\tsubq $16, %rsp")?;
                if *t == AsmType::Double {
                    writeln!(w, "\tmovsd {}, (%rsp)", show_operand(src, *t, platform)?)?;
                } else if *t == AsmType::Float {
                    writeln!(w, "\tmovss {}, (%rsp)", show_operand(src, *t, platform)?)?;
                } else {
                    writeln!(
                        w,
                        "\tmov{} {}, (%rsp)",
                        suffix(*t),
                        show_operand(src, *t, platform)?
                    )?;
                }
                writeln!(w, "\t{} (%rsp)", mnemonic)?;
                return writeln!(w, "\taddq $16, %rsp");
            }
            writeln!(w, "\t{} {}", mnemonic, show_operand(src, *t, platform)?)
        }
        AsmInstr::X87Store(dst) => {
            writeln!(
                w,
                "\tfstpt {}",
                show_operand(dst, AsmType::LongDouble, platform)?
            )
        }
        AsmInstr::X87StoreFloat(t, dst) => {
            let mnemonic = match t {
                AsmType::Float => "fstps",
                AsmType::Double => "fstpl",
                other => {
                    return invalid_input(format!(
                        "unsupported x87 floating store type in x86-64 emitter: {:?}",
                        other
                    ))
                }
            };
            writeln!(w, "\t{} {}", mnemonic, show_operand(dst, *t, platform)?)
        }
        AsmInstr::X87StoreInt(t, dst) => {
            let mnemonic = match t {
                AsmType::Word => "fisttps",
                AsmType::Longword => "fisttpl",
                AsmType::Quadword => "fisttpq",
                other => {
                    return invalid_input(format!(
                        "unsupported x87 integer store type in x86-64 emitter: {:?}",
                        other
                    ))
                }
            };
            if matches!(dst, AsmOperand::Reg(_)) {
                writeln!(w, "\tsubq $16, %rsp")?;
                writeln!(w, "\t{} (%rsp)", mnemonic)?;
                writeln!(
                    w,
                    "\tmov{} (%rsp), {}",
                    suffix(*t),
                    show_operand(dst, *t, platform)?
                )?;
                return writeln!(w, "\taddq $16, %rsp");
            }
            writeln!(w, "\t{} {}", mnemonic, show_operand(dst, *t, platform)?)
        }
        AsmInstr::X87LoadIndirect(t, reg) => {
            let mnemonic = match t {
                AsmType::Float => "flds",
                AsmType::Double => "fldl",
                AsmType::LongDouble => "fldt",
                other => {
                    return invalid_input(format!(
                        "unsupported x87 indirect load type in x86-64 emitter: {:?}",
                        other
                    ))
                }
            };
            writeln!(w, "\t{} ({})", mnemonic, reg_name(reg, AsmType::Quadword)?)
        }
        AsmInstr::X87StoreIndirect(reg) => {
            writeln!(w, "\tfstpt ({})", reg_name(reg, AsmType::Quadword)?)
        }
        AsmInstr::X87UnaryNeg => writeln!(w, "\tfchs"),
        AsmInstr::X87Binary(op) => {
            let mnemonic = match op {
                AsmX87BinaryOp::Add => "faddp",
                AsmX87BinaryOp::Sub => "fsubrp",
                AsmX87BinaryOp::Mul => "fmulp",
                AsmX87BinaryOp::Div => "fdivrp",
                AsmX87BinaryOp::Cmp => "fucomip",
            };
            writeln!(w, "\t{} %st, %st(1)", mnemonic)
        }
        AsmInstr::X87Compare => {
            writeln!(w, "\tfucomip %st(1), %st")?;
            writeln!(w, "\tfstp %st(0)")
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
                (AsmType::Byte, AsmType::Byte) => "movb",
                (AsmType::Byte, AsmType::Word) => "movsbw",
                (AsmType::Byte, AsmType::Longword) => "movsbl",
                (AsmType::Byte, AsmType::Quadword) => "movsbq",
                (AsmType::Word, AsmType::Word) => "movw",
                (AsmType::Word, AsmType::Longword) => "movswl",
                (AsmType::Word, AsmType::Quadword) => "movswq",
                (AsmType::Longword, AsmType::Longword) => "movl",
                (AsmType::Longword, AsmType::Quadword) => "movslq",
                (_, AsmType::Byte) => "movb",
                (_, AsmType::Word) => "movw",
                (_, AsmType::Longword) => "movl",
                (_, AsmType::Quadword) => "movq",
                _ => return invalid_input("unsupported x86-64 sign-extension conversion"),
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
                (AsmType::Byte, AsmType::Word) => "movzbw",
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
                        let reg_type = if *dst_t == AsmType::Quadword
                            && matches!(*src_t, AsmType::Byte | AsmType::Word)
                        {
                            AsmType::Quadword
                        } else {
                            AsmType::Longword
                        };
                        writeln!(
                            w,
                            "\t{} {}, {}",
                            mnemonic,
                            show_operand(src, *src_t, platform)?,
                            reg_name(reg, reg_type)?
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
                AsmBinaryOp::Add | AsmBinaryOp::AddSetFlags => "add",
                AsmBinaryOp::Adc => "adc",
                AsmBinaryOp::Sub => "sub",
                AsmBinaryOp::SubSetFlags => "sub",
                AsmBinaryOp::Sbb => "sbb",
                AsmBinaryOp::Mul => "imul",
                AsmBinaryOp::Imul => "imul",
                AsmBinaryOp::SDiv | AsmBinaryOp::UDiv => {
                    return invalid_input("AArch64 integer division op reached x86_64 emitter")
                }
                AsmBinaryOp::DivDouble => {
                    return invalid_input("DivDouble should only be used with Double type")
                }
                AsmBinaryOp::Div => "div",
                AsmBinaryOp::Idiv => "idiv",
                AsmBinaryOp::And => "and",
                AsmBinaryOp::Nand => {
                    return invalid_input("Nand should only be used by atomic RMW")
                }
                AsmBinaryOp::Or => "or",
                AsmBinaryOp::Xor => "xor",
                AsmBinaryOp::Sal => "sal",
                AsmBinaryOp::Sar => "sar",
                AsmBinaryOp::Shr => "shr",
                AsmBinaryOp::Cmp => "cmp",
                AsmBinaryOp::Test => "test",
                AsmBinaryOp::SetCC => "setcc",
            };
            match op {
                AsmBinaryOp::Sal | AsmBinaryOp::Sar | AsmBinaryOp::Shr => {
                    let shift_src = match src {
                        AsmOperand::Reg(Reg::CX) => "%cl".to_string(),
                        // The x86 immediate shift encoding holds only an
                        // 8-bit count, and the processor masks that count by
                        // the operand width. Normalize here so out-of-range
                        // C shift counts still produce assemblable code with
                        // the same behavior as a register count.
                        AsmOperand::Imm(val) => {
                            let mask = if *t == AsmType::Quadword { 63 } else { 31 };
                            format!("${}", val & mask)
                        }
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
                            if !fits_sign_extended_i32(*v) {
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
                            if !fits_sign_extended_i32(*v) {
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
        AsmInstr::MulFull(t, operand) => {
            let mnemonic = match t {
                AsmType::Longword => "mull",
                AsmType::Quadword => "mulq",
                _ => return invalid_input("full multiply requires an integer type"),
            };
            writeln!(w, "\t{} {}", mnemonic, show_operand(operand, *t, platform)?)
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
                // `test operand, operand` sets the same flags as `cmp $0,
                // operand` for every condition code used by the backend, but
                // avoids materializing an immediate zero.
                if matches!(src, AsmOperand::Imm(0)) && matches!(dst, AsmOperand::Reg(_)) {
                    let operand = show_operand(dst, *t, platform)?;
                    return writeln!(w, "\ttest{} {}, {}", suffix(*t), operand, operand);
                }
                // cmpq doesn't support 64-bit immediates
                if *t == AsmType::Quadword {
                    if let AsmOperand::Imm(v) = src {
                        if !fits_sign_extended_i32(*v) {
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
                        if !fits_sign_extended_i32(*v) {
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
        AsmInstr::CopyToStackArg {
            src_ptr,
            dst_offset,
            size,
        } => {
            writeln!(
                w,
                "\tmovq {}, %rsi",
                show_operand(src_ptr, AsmType::Quadword, platform)?
            )?;
            writeln!(w, "\tleaq {}(%rsp), %rdi", dst_offset)?;
            writeln!(w, "\tmovq ${}, %rcx", size)?;
            writeln!(w, "\trep movsb")
        }
        AsmInstr::CopyFromStackArg {
            src_offset,
            dst,
            size,
        } => {
            writeln!(w, "\tleaq {}(%rbp), %rsi", src_offset)?;
            writeln!(
                w,
                "\tleaq {}, %rdi",
                show_operand(dst, AsmType::Quadword, platform)?
            )?;
            writeln!(w, "\tmovq ${}, %rcx", size)?;
            writeln!(w, "\trep movsb")
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
        AsmInstr::NonlocalJmp(label) => {
            writeln!(w, "\tmovq %rbp, %rsp")?;
            writeln!(w, "\tpopq %rbp")?;
            writeln!(w, "\taddq $8, %rsp")?;
            writeln!(w, "\tjmp .L{}", label)
        }
        AsmInstr::JmpIndirect(target) => writeln!(
            w,
            "\tjmp *{}",
            show_operand(target, AsmType::Quadword, platform)?
        ),
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
        AsmInstr::LoadLabelAddress(label, dst) => writeln!(
            w,
            "\tleaq .L{}(%rip), {}",
            label,
            show_operand(dst, AsmType::Quadword, platform)?
        ),
        AsmInstr::BuiltinSetjmp {
            buf,
            dst,
            label,
            end_label,
        } => {
            writeln!(
                w,
                "\tmovq {}, %r11",
                show_operand(buf, AsmType::Quadword, platform)?
            )?;
            writeln!(w, "\tleaq .L{}(%rip), %r10", label)?;
            writeln!(w, "\tmovq %r10, (%r11)")?;
            writeln!(w, "\tmovq %rsp, 8(%r11)")?;
            writeln!(w, "\tmovq %rbp, 16(%r11)")?;
            writeln!(
                w,
                "\tmovl $0, {}",
                show_operand(dst, AsmType::Longword, platform)?
            )?;
            writeln!(w, "\tjmp .L{}", end_label)?;
            writeln!(w, ".L{}:", label)?;
            writeln!(
                w,
                "\tmovl $1, {}",
                show_operand(dst, AsmType::Longword, platform)?
            )?;
            writeln!(w, ".L{}:", end_label)
        }
        AsmInstr::BuiltinLongjmp { buf, value: _ } => {
            writeln!(
                w,
                "\tmovq {}, %r11",
                show_operand(buf, AsmType::Quadword, platform)?
            )?;
            writeln!(w, "\tmovq 8(%r11), %rsp")?;
            writeln!(w, "\tmovq 16(%r11), %rbp")?;
            writeln!(w, "\tjmp *(%r11)")
        }
        AsmInstr::Push(operand) => {
            // pushq doesn't support XMM registers
            if let AsmOperand::Xmm(xmm) = operand {
                writeln!(w, "\tsubq $8, %rsp")?;
                return writeln!(w, "\tmovsd {}, (%rsp)", xmm_name(xmm));
            }
            // pushq doesn't support 64-bit immediates
            if let AsmOperand::Imm(v) = operand {
                if !fits_sign_extended_i32(*v) {
                    writeln!(w, "\tmovq ${}, %r10", v)?;
                    return writeln!(w, "\tpushq %r10");
                }
            }
            writeln!(w, "\tpushq {}", show_operand_64(operand, platform)?)
        }
        AsmInstr::Pop(reg) => {
            writeln!(w, "\tpopq {}", reg_name(reg, AsmType::Quadword)?)
        }
        AsmInstr::Call(name, _, _, indirect, local) => {
            if *indirect {
                // Indirect call through R10 (function pointer already loaded there)
                writeln!(w, "\tcall *%r10")
            } else {
                let label = platform.show_symbol(name);
                match platform.os {
                    TargetOs::MacOs => writeln!(w, "\tcall {}", label),
                    TargetOs::Linux if *local => writeln!(w, "\tcall {}", label),
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
                if *size > i32::MAX as i64 {
                    writeln!(w, "\tmovq ${}, %r10", size)?;
                    writeln!(w, "\tsubq %r10, %rsp")
                } else {
                    writeln!(w, "\tsubq ${}, %rsp", size)
                }
            } else {
                Ok(())
            }
        }
        AsmInstr::DeallocateStack(size) => {
            if *size > 0 {
                if *size > i32::MAX as i64 {
                    writeln!(w, "\tmovq ${}, %r10", size)?;
                    writeln!(w, "\taddq %r10, %rsp")
                } else {
                    writeln!(w, "\taddq ${}, %rsp", size)
                }
            } else {
                Ok(())
            }
        }
        AsmInstr::AArch64AllocateLargeStack(_)
        | AsmInstr::AArch64DeallocateLargeStack(_)
        | AsmInstr::AArch64Extr(_, _, _, _)
        | AsmInstr::AArch64Umulh(_, _, _)
        | AsmInstr::AArch64StoreLargeLocalBase { .. } => invalid_input(format!(
            "x86-64 backend cannot emit AArch64 instruction: {:?}",
            instr
        )),
        AsmInstr::And(_, _, _)
        | AsmInstr::Or(_, _, _)
        | AsmInstr::Xor(_, _, _)
        | AsmInstr::Test(_, _, _)
        | AsmInstr::Shl(_, _, _)
        | AsmInstr::Shr(_, _, _)
        | AsmInstr::Sar(_, _, _)
        | AsmInstr::Ror(_, _, _)
        | AsmInstr::Rol(_, _, _)
        | AsmInstr::Fld(_, _)
        | AsmInstr::Fstp(_, _)
        | AsmInstr::Fisttp(_, _)
        | AsmInstr::Fxch
        | AsmInstr::FstpQ
        | AsmInstr::FldQ(_)
        | AsmInstr::X87Push(_, _)
        | AsmInstr::X87Pop(_, _) => invalid_input(format!(
            "x86-64 backend cannot emit instruction: {:?}",
            instr
        )),
    }
}

/// Instructions that READ the condition-code flags. Used to decide whether the
/// `mov $0, reg` -> `xor reg, reg` size optimization is safe: `xor` clobbers the
/// flags, so it must not be substituted while a flag reader is still pending
/// (e.g. the `cmp; mov $0, dst; setcc dst` comparison-lowering sequence).
fn x86_reads_flags(instr: &AsmInstr) -> bool {
    matches!(
        instr,
        AsmInstr::SetCC(..)
            | AsmInstr::JmpCC(..)
            | AsmInstr::Binary(
                _,
                AsmBinaryOp::Adc | AsmBinaryOp::Sbb | AsmBinaryOp::SetCC,
                _,
                _
            )
    )
}

/// Instructions that DEFINITELY overwrite the flags. Once one of these is
/// reached (before any flag reader) the earlier flag state is dead, so zeroing
/// with `xor` is safe. Kept to a subset of true flag writers so we never treat
/// a flag-preserving instruction (e.g. `not`) as a writer.
fn x86_writes_flags(instr: &AsmInstr) -> bool {
    match instr {
        AsmInstr::Cmp(..) | AsmInstr::Test(..) => true,
        AsmInstr::Idiv(..) | AsmInstr::Div(..) | AsmInstr::MulFull(..) => true,
        AsmInstr::Unary(_, AsmUnaryOp::Neg, _) => true,
        // Integer ALU binaries set flags; `not` (a Unary) and float division do not.
        AsmInstr::Binary(_, op, _, _) => !matches!(op, AsmBinaryOp::DivDouble),
        _ => false,
    }
}

/// Decide whether `mov $0, reg` at index `i` may be emitted as `xor reg, reg`.
/// Scans forward until the flag state is proven dead (a flag writer, a `ret`, or
/// a call — which clobbers flags per the SysV ABI) or proven live (a flag
/// reader). Any other control-flow transfer (jump/label/setjmp) is treated
/// conservatively as "flags may be live" so the flags-clobbering `xor` is not
/// used across a boundary whose downstream flag usage we cannot see here.
fn zeroing_can_use_xor(instrs: &[AsmInstr], i: usize) -> bool {
    for instr in &instrs[i + 1..] {
        if x86_reads_flags(instr) {
            return false;
        }
        if x86_writes_flags(instr) {
            return true;
        }
        match instr {
            AsmInstr::Ret | AsmInstr::Call(..) => return true,
            AsmInstr::Jmp(_)
            | AsmInstr::NonlocalJmp(_)
            | AsmInstr::JmpIndirect(_)
            | AsmInstr::Label(_)
            | AsmInstr::BuiltinSetjmp { .. }
            | AsmInstr::BuiltinLongjmp { .. }
            | AsmInstr::Unreachable => return false,
            _ => {}
        }
    }
    true
}

fn emit_zeroing_mov(
    w: &mut dyn Write,
    t: AsmType,
    reg: &Reg,
    use_xor: bool,
) -> std::io::Result<()> {
    let name = reg_name(reg, t)?;
    if use_xor {
        writeln!(w, "\txor{} {}, {}", suffix(t), name, name)
    } else {
        writeln!(w, "\tmov{} $0, {}", suffix(t), name)
    }
}

/// A 32-bit x86 write already clears the corresponding register's upper half.
/// Consequently a following `movl %reg, %reg` cannot add useful
/// zero-extension when the immediately preceding instruction is a known
/// 32-bit ALU write to that same register.
fn longword_self_move_is_redundant(instrs: &[AsmInstr], index: usize) -> bool {
    let Some(AsmInstr::Mov(AsmType::Longword, AsmOperand::Reg(reg), AsmOperand::Reg(dst_reg))) =
        instrs.get(index)
    else {
        return false;
    };
    if reg != dst_reg || index == 0 {
        return false;
    }

    match &instrs[index - 1] {
        AsmInstr::Mov(AsmType::Longword, _, AsmOperand::Reg(written_reg)) => written_reg == reg,
        AsmInstr::Binary(AsmType::Longword, op, _, AsmOperand::Reg(written_reg)) => {
            written_reg == reg
                && matches!(
                    op,
                    AsmBinaryOp::Add
                        | AsmBinaryOp::AddSetFlags
                        | AsmBinaryOp::Adc
                        | AsmBinaryOp::Sub
                        | AsmBinaryOp::SubSetFlags
                        | AsmBinaryOp::Sbb
                        | AsmBinaryOp::Mul
                        | AsmBinaryOp::Imul
                        | AsmBinaryOp::And
                        | AsmBinaryOp::Or
                        | AsmBinaryOp::Xor
                        | AsmBinaryOp::Sal
                        | AsmBinaryOp::Sar
                        | AsmBinaryOp::Shr
                )
        }
        AsmInstr::LoadIndirect(AsmType::Longword, _, AsmOperand::Reg(written_reg)) => {
            written_reg == reg
        }
        _ => false,
    }
}

fn invert_condition(cc: &CondCode) -> CondCode {
    match cc {
        CondCode::E => CondCode::NE,
        CondCode::NE => CondCode::E,
        CondCode::L => CondCode::GE,
        CondCode::LE => CondCode::G,
        CondCode::G => CondCode::LE,
        CondCode::GE => CondCode::L,
        CondCode::A => CondCode::BE,
        CondCode::AE => CondCode::B,
        CondCode::B => CondCode::AE,
        CondCode::BE => CondCode::A,
        CondCode::P => CondCode::NP,
        CondCode::NP => CondCode::P,
        CondCode::S => CondCode::NS,
        CondCode::NS => CondCode::S,
    }
}

/// Fuse `cmp; mov $0; setcc; test; j{e,ne}` when the boolean has no other
/// use.  The zeroing move preserves the flags for `setcc`; the final test only
/// branches on that 0/1 result, so the original comparison can branch directly.
fn fused_setcc_branch(instrs: &[AsmInstr], index: usize) -> Option<(CondCode, String)> {
    if !matches!(instrs.get(index), Some(AsmInstr::Cmp(..))) {
        return None;
    }
    let AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(0), AsmOperand::Reg(reg)) =
        instrs.get(index + 1)?
    else {
        return None;
    };
    let AsmInstr::SetCC(set_cc, AsmOperand::Reg(set_reg)) = instrs.get(index + 2)? else {
        return None;
    };
    let AsmInstr::Cmp(AsmType::Longword, AsmOperand::Imm(0), AsmOperand::Reg(cmp_reg)) =
        instrs.get(index + 3)?
    else {
        return None;
    };
    let AsmInstr::JmpCC(branch_cc, label) = instrs.get(index + 4)? else {
        return None;
    };
    if reg != set_reg || reg != cmp_reg {
        return None;
    }
    let cc = match branch_cc {
        CondCode::E => invert_condition(set_cc),
        CondCode::NE => *set_cc,
        _ => return None,
    };
    Some((cc, label.clone()))
}

fn emit_function(w: &mut dyn Write, func: &AsmFunction, platform: &Target) -> std::io::Result<()> {
    let label = platform.show_symbol(&func.name);
    writeln!(w, "\t.text")?;
    if func.global {
        writeln!(w, "\t.globl {}", label)?;
    }
    writeln!(w, "{}:", label)?;
    let instrs = &func.instructions;
    let mut start = 0;
    if let Some(AsmInstr::Push(AsmOperand::Reg(Reg::AX))) = instrs.first() {
        writeln!(w, "\tpushq %rbp")?;
        writeln!(w, "\tmovq %rsp, %rbp")?;
        start = 1;
    }
    let mut idx = start;
    while idx < instrs.len() {
        let instr = &instrs[idx];
        if let Some((cc, label)) = fused_setcc_branch(instrs, idx) {
            emit_instruction(w, instr, platform)?;
            emit_instruction(w, &AsmInstr::JmpCC(cc, label), platform)?;
            idx += 5;
            continue;
        }
        if longword_self_move_is_redundant(instrs, idx) {
            idx += 1;
            continue;
        }
        // `mov $0, reg` can shrink to `xor reg, reg`, but only where the flags
        // it would clobber are dead (see `zeroing_can_use_xor`).
        if let AsmInstr::Mov(t, AsmOperand::Imm(0), AsmOperand::Reg(reg)) = instr {
            if !matches!(*t, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
                emit_zeroing_mov(w, *t, reg, zeroing_can_use_xor(instrs, idx))?;
                idx += 1;
                continue;
            }
        }
        emit_instruction(w, instr, platform)?;
        idx += 1;
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
        StaticInit::LabelDiffInit(_, _, bytes) => *bytes,
        StaticInit::Int128Init(_) | StaticInit::UInt128Init(_) | StaticInit::LongDoubleInit(_) => {
            16
        }
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
    let label = platform.show_symbol(&sv.name);
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
    let label = platform.show_symbol(&sv.name);
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

fn emit_string_init(w: &mut dyn Write, s: &str, null_terminated: bool) -> std::io::Result<()> {
    let string_bytes = c_string_bytes(s);
    if string_bytes.contains(&0) {
        let mut bytes = string_bytes;
        if null_terminated {
            bytes.push(0);
        }
        for chunk in bytes.chunks(16) {
            write!(w, "\t.byte ")?;
            for (idx, byte) in chunk.iter().enumerate() {
                if idx > 0 {
                    write!(w, ", ")?;
                }
                write!(w, "{byte}")?;
            }
            writeln!(w)?;
        }
        Ok(())
    } else {
        let escaped = escape_string_for_asm(s);
        if null_terminated {
            writeln!(w, "\t.asciz \"{}\"", escaped)
        } else {
            writeln!(w, "\t.ascii \"{}\"", escaped)
        }
    }
}

fn x87_long_double_bytes(value: f64) -> [u8; 16] {
    let bits = value.to_bits();
    let sign = ((bits >> 63) as u16) << 15;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = bits & ((1u64 << 52) - 1);
    let (significand, exponent) = if exp == 0 {
        if frac == 0 {
            (0, 0)
        } else {
            let top_bit = 63 - frac.leading_zeros() as i32;
            let significand = frac << (63 - top_bit);
            let unbiased = top_bit - 1074;
            (significand, unbiased + 16383)
        }
    } else if exp == 0x7ff {
        let significand = if frac == 0 {
            1u64 << 63
        } else {
            (1u64 << 63) | (frac << 11)
        };
        (significand, 0x7fff)
    } else {
        let unbiased = exp - 1023;
        ((1u64 << 63) | (frac << 11), unbiased + 16383)
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&significand.to_le_bytes());
    bytes[8..10].copy_from_slice(&(sign | exponent as u16).to_le_bytes());
    bytes
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
        StaticInit::Int128Init(v) => {
            writeln!(w, "\t.quad {}", *v as i64)?;
            writeln!(w, "\t.quad {}", (*v >> 64) as i64)
        }
        StaticInit::UInt128Init(v) => {
            writeln!(w, "\t.quad {}", *v as u64)?;
            writeln!(w, "\t.quad {}", (*v >> 64) as u64)
        }
        StaticInit::FloatInit(v) => writeln!(w, "\t.long {}", v.to_bits()),
        StaticInit::DoubleInit(v) => writeln!(w, "\t.quad {}", v.to_bits()),
        StaticInit::LongDoubleInit(v) => {
            for byte in x87_long_double_bytes(*v) {
                writeln!(w, "\t.byte {}", byte)?;
            }
            Ok(())
        }
        StaticInit::ZeroInit(n) => writeln!(w, "\t.zero {}", n),
        StaticInit::StringInit(s, null_terminated) => emit_string_init(w, s, *null_terminated),
        StaticInit::PointerInit(label) => {
            writeln!(w, "\t.quad {}", static_label_name(platform, label))
        }
        StaticInit::PointerInitOffset(label, offset) => {
            writeln!(
                w,
                "\t.quad {}{}",
                static_label_name(platform, label),
                assembly_offset_suffix(*offset)
            )
        }
        StaticInit::LabelDiffInit(left, right, 1) => writeln!(
            w,
            "\t.byte {}-{}",
            static_label_name(platform, left),
            static_label_name(platform, right)
        ),
        StaticInit::LabelDiffInit(left, right, 2) => writeln!(
            w,
            "\t.short {}-{}",
            static_label_name(platform, left),
            static_label_name(platform, right)
        ),
        StaticInit::LabelDiffInit(left, right, 4) => writeln!(
            w,
            "\t.long {}-{}",
            static_label_name(platform, left),
            static_label_name(platform, right)
        ),
        StaticInit::LabelDiffInit(left, right, 8) => writeln!(
            w,
            "\t.quad {}-{}",
            static_label_name(platform, left),
            static_label_name(platform, right)
        ),
        StaticInit::LabelDiffInit(_, _, bytes) => invalid_input(format!(
            "unsupported label difference initializer size: {bytes}"
        )),
    }
}

fn emit_static_constant(
    w: &mut dyn Write,
    sc: &AsmStaticConstant,
    platform: &Target,
) -> std::io::Result<()> {
    let label = platform.show_symbol(&sc.name);
    match platform.os {
        TargetOs::MacOs if matches!(&sc.init, StaticInit::StringInit(s, _) if !c_string_bytes(s).contains(&0)) => {
            writeln!(w, "\t.section __TEXT,__cstring")?
        }
        TargetOs::MacOs => writeln!(w, "\t.section __TEXT,__const")?,
        TargetOs::Linux => writeln!(w, "\t.section .rodata")?,
    }
    if sc.alignment > 1 {
        writeln!(w, "\t.balign {}", sc.alignment)?;
    }
    writeln!(w, "{}:", label)?;
    emit_static_init(w, &sc.init, platform)?;
    Ok(())
}

fn emit_alias(
    w: &mut dyn Write,
    name: &str,
    alias_target: &str,
    platform: &Target,
) -> std::io::Result<()> {
    if alias_target.is_empty() {
        return Ok(());
    }
    let name = platform.show_symbol(name);
    let alias_target = platform.show_symbol(alias_target);
    writeln!(w, "\t.globl {}", name)?;
    writeln!(w, "\t.set {}, {}", name, alias_target)
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
            AsmTopLevel::Alias { name, target } => emit_alias(&mut w, name, target, platform)?,
        }
    }
    emit_stack_note(&mut w, platform)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn emit_one(instr: AsmInstr) -> String {
        let mut out = Vec::new();
        emit_instruction(&mut out, &instr, &Target::x86_64_linux()).unwrap();
        String::from_utf8(out).unwrap()
    }

    fn emit_func_body(instrs: Vec<AsmInstr>) -> String {
        let func = AsmFunction {
            name: "f".to_string(),
            global: false,
            instructions: instrs,
        };
        let mut out = Vec::new();
        emit_function(&mut out, &func, &Target::x86_64_linux()).unwrap();
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn x86_64_emitter_skips_non_widening_self_moves() {
        let asm = emit_one(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Reg(Reg::AX),
            AsmOperand::Reg(Reg::AX),
        ));

        assert!(asm.is_empty(), "{asm}");
    }

    #[test]
    fn x86_64_emitter_keeps_longword_self_moves_for_zero_extension() {
        let asm = emit_one(AsmInstr::Mov(
            AsmType::Longword,
            AsmOperand::Reg(Reg::AX),
            AsmOperand::Reg(Reg::AX),
        ));

        assert_eq!(asm, "\tmovl %eax, %eax\n");
    }

    #[test]
    fn x86_64_emitter_drops_self_move_after_known_longword_write() {
        let asm = emit_func_body(vec![
            AsmInstr::Binary(
                AsmType::Longword,
                AsmBinaryOp::Sub,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::DI),
            ),
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Reg(Reg::DI),
                AsmOperand::Reg(Reg::DI),
            ),
            AsmInstr::Ret,
        ]);

        assert!(asm.contains("\tsubl $1, %edi\n"), "{asm}");
        assert!(!asm.contains("\tmovl %edi, %edi\n"), "{asm}");
    }

    #[test]
    fn x86_64_emitter_fuses_setcc_boolean_branch() {
        let asm = emit_func_body(vec![
            AsmInstr::Cmp(
                AsmType::Longword,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::DX),
            ),
            AsmInstr::SetCC(CondCode::LE, AsmOperand::Reg(Reg::DX)),
            AsmInstr::Cmp(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::DX),
            ),
            AsmInstr::JmpCC(CondCode::E, "false".to_string()),
            AsmInstr::Label("false".to_string()),
            AsmInstr::Ret,
        ]);

        assert!(asm.contains("\tcmpl $1, %eax\n"), "{asm}");
        assert!(asm.contains("\tjg .Lfalse\n"), "{asm}");
        assert!(!asm.contains("setle"), "{asm}");
        assert!(!asm.contains("testl %edx, %edx"), "{asm}");
    }

    #[test]
    fn x86_64_emitter_zeroing_defaults_to_mov_without_flag_context() {
        // A bare zeroing move, with no following instruction to prove the flags
        // are dead, must use the flag-preserving `mov $0` form.
        let asm = emit_one(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Imm(0),
            AsmOperand::Reg(Reg::AX),
        ));

        assert_eq!(asm, "\tmovq $0, %rax\n");
    }

    #[test]
    fn x86_64_emitter_zeros_with_xor_when_flags_are_dead() {
        // `mov $0, %eax` followed by `ret` (flags dead at exit) -> `xor`.
        let asm = emit_func_body(vec![
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Ret,
        ]);

        assert!(asm.contains("\txorl %eax, %eax\n"), "{asm}");
    }

    #[test]
    fn x86_64_emitter_keeps_mov_when_setcc_reads_flags() {
        // The comparison-lowering sequence: the zeroing sits between `cmp` and
        // `setcc`, which reads the flags. `xor` here would corrupt the result,
        // so the flag-preserving `mov $0` must be kept.
        let asm = emit_func_body(vec![
            AsmInstr::Cmp(
                AsmType::Longword,
                AsmOperand::Imm(2),
                AsmOperand::Reg(Reg::R11),
            ),
            AsmInstr::Mov(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::SetCC(CondCode::E, AsmOperand::Reg(Reg::AX)),
            AsmInstr::Ret,
        ]);

        assert!(asm.contains("\tmovl $0, %eax\n"), "{asm}");
        assert!(
            !asm.contains("xor"),
            "must not clobber flags before setcc: {asm}"
        );
    }

    #[test]
    fn x86_64_emitter_masks_large_shift_immediates() {
        let quadword = emit_one(AsmInstr::Binary(
            AsmType::Quadword,
            AsmBinaryOp::Sal,
            AsmOperand::Imm(671111),
            AsmOperand::Reg(Reg::AX),
        ));
        assert_eq!(quadword, "\tsalq $7, %rax\n");

        let longword = emit_one(AsmInstr::Binary(
            AsmType::Longword,
            AsmBinaryOp::Shr,
            AsmOperand::Imm(65),
            AsmOperand::Reg(Reg::AX),
        ));
        assert_eq!(longword, "\tshrl $1, %eax\n");
    }

    #[test]
    fn x86_64_emitter_uses_test_for_integer_compare_against_zero() {
        let longword = emit_one(AsmInstr::Cmp(
            AsmType::Longword,
            AsmOperand::Imm(0),
            AsmOperand::Reg(Reg::AX),
        ));
        assert_eq!(longword, "\ttestl %eax, %eax\n");

        let quadword = emit_one(AsmInstr::Cmp(
            AsmType::Quadword,
            AsmOperand::Imm(0),
            AsmOperand::Reg(Reg::R11),
        ));
        assert_eq!(quadword, "\ttestq %r11, %r11\n");

        let memory = emit_one(AsmInstr::Cmp(
            AsmType::Longword,
            AsmOperand::Imm(0),
            AsmOperand::Stack(-4),
        ));
        assert_eq!(memory, "\tcmpl $0, -4(%rbp)\n");
    }
}
