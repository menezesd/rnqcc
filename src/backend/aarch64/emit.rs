use crate::types::*;
use std::io::{self, Write};

fn invalid_input<T>(message: impl Into<String>) -> io::Result<T> {
    Err(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn static_label_name(target: &Target, label: &str) -> String {
    if label.starts_with("label.") {
        format!(".L{}", label)
    } else {
        target.show_symbol(label)
    }
}

fn reg_name(reg: Reg, ty: AsmType) -> io::Result<&'static str> {
    match (reg, ty) {
        (Reg::AX, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w0"),
        (Reg::AX, AsmType::Quadword) => Ok("x0"),
        (Reg::DI, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w1"),
        (Reg::DI, AsmType::Quadword) => Ok("x1"),
        (Reg::SI, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w2"),
        (Reg::SI, AsmType::Quadword) => Ok("x2"),
        (Reg::DX, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w3"),
        (Reg::DX, AsmType::Quadword) => Ok("x3"),
        (Reg::CX, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w4"),
        (Reg::CX, AsmType::Quadword) => Ok("x4"),
        (Reg::R8, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w5"),
        (Reg::R8, AsmType::Quadword) => Ok("x5"),
        (Reg::R9, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w6"),
        (Reg::R9, AsmType::Quadword) => Ok("x6"),
        (Reg::R12, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w7"),
        (Reg::R12, AsmType::Quadword) => Ok("x7"),
        (Reg::R13, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w11"),
        (Reg::R13, AsmType::Quadword) => Ok("x11"),
        (Reg::R14, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w12"),
        (Reg::R14, AsmType::Quadword) => Ok("x12"),
        (Reg::R15, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w13"),
        (Reg::R15, AsmType::Quadword) => Ok("x13"),
        (Reg::BP, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w29"),
        (Reg::BP, AsmType::Quadword) => Ok("x29"),
        (Reg::R10, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w9"),
        (Reg::R10, AsmType::Quadword) => Ok("x9"),
        (Reg::R11, AsmType::Byte | AsmType::Word | AsmType::Longword) => Ok("w10"),
        (Reg::R11, AsmType::Quadword) => Ok("x10"),
        (_, AsmType::Float | AsmType::Double | AsmType::LongDouble) => {
            invalid_input("AArch64 integer register requested for floating type")
        }
        _ => invalid_input(format!(
            "AArch64 backend does not map register {:?} yet",
            reg
        )),
    }
}

fn fp_name(reg: XmmReg) -> &'static str {
    match reg {
        XmmReg::XMM0 => "d0",
        XmmReg::XMM1 => "d1",
        XmmReg::XMM2 => "d2",
        XmmReg::XMM3 => "d3",
        XmmReg::XMM4 => "d4",
        XmmReg::XMM5 => "d5",
        XmmReg::XMM6 => "d6",
        XmmReg::XMM7 => "d7",
        XmmReg::XMM8 => "d8",
        XmmReg::XMM9 => "d9",
        XmmReg::XMM10 => "d10",
        XmmReg::XMM11 => "d11",
        XmmReg::XMM12 => "d12",
        XmmReg::XMM13 => "d13",
        XmmReg::XMM14 => "d14",
        XmmReg::XMM15 => "d15",
    }
}

fn fp_vector_name(reg: XmmReg) -> &'static str {
    match reg {
        XmmReg::XMM0 => "v0",
        XmmReg::XMM1 => "v1",
        XmmReg::XMM2 => "v2",
        XmmReg::XMM3 => "v3",
        XmmReg::XMM4 => "v4",
        XmmReg::XMM5 => "v5",
        XmmReg::XMM6 => "v6",
        XmmReg::XMM7 => "v7",
        XmmReg::XMM8 => "v8",
        XmmReg::XMM9 => "v9",
        XmmReg::XMM10 => "v10",
        XmmReg::XMM11 => "v11",
        XmmReg::XMM12 => "v12",
        XmmReg::XMM13 => "v13",
        XmmReg::XMM14 => "v14",
        XmmReg::XMM15 => "v15",
    }
}

fn q_reg_to_vector_name(reg: &str) -> io::Result<String> {
    reg.strip_prefix('q')
        .map(|num| format!("v{}.16b", num))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("AArch64 expected q register for long double, got {}", reg),
            )
        })
}

fn fp_name_typed(reg: XmmReg, ty: AsmType) -> io::Result<&'static str> {
    match ty {
        AsmType::Float => Ok(match reg {
            XmmReg::XMM0 => "s0",
            XmmReg::XMM1 => "s1",
            XmmReg::XMM2 => "s2",
            XmmReg::XMM3 => "s3",
            XmmReg::XMM4 => "s4",
            XmmReg::XMM5 => "s5",
            XmmReg::XMM6 => "s6",
            XmmReg::XMM7 => "s7",
            XmmReg::XMM8 => "s8",
            XmmReg::XMM9 => "s9",
            XmmReg::XMM10 => "s10",
            XmmReg::XMM11 => "s11",
            XmmReg::XMM12 => "s12",
            XmmReg::XMM13 => "s13",
            XmmReg::XMM14 => "s14",
            XmmReg::XMM15 => "s15",
        }),
        AsmType::LongDouble => Ok(match reg {
            XmmReg::XMM0 => "q0",
            XmmReg::XMM1 => "q1",
            XmmReg::XMM2 => "q2",
            XmmReg::XMM3 => "q3",
            XmmReg::XMM4 => "q4",
            XmmReg::XMM5 => "q5",
            XmmReg::XMM6 => "q6",
            XmmReg::XMM7 => "q7",
            XmmReg::XMM8 => "q8",
            XmmReg::XMM9 => "q9",
            XmmReg::XMM10 => "q10",
            XmmReg::XMM11 => "q11",
            XmmReg::XMM12 => "q12",
            XmmReg::XMM13 => "q13",
            XmmReg::XMM14 => "q14",
            XmmReg::XMM15 => "q15",
        }),
        _ => Ok(fp_name(reg)),
    }
}

fn fp_scratch_name_typed(reg: Reg, ty: AsmType) -> io::Result<&'static str> {
    if ty == AsmType::Float {
        return match reg {
            Reg::R10 => Ok("s9"),
            Reg::R11 => Ok("s10"),
            Reg::R13 => Ok("s11"),
            _ => invalid_input(format!(
                "AArch64 backend does not map {:?} as FP scratch",
                reg
            )),
        };
    }
    if ty == AsmType::LongDouble {
        return match reg {
            Reg::R10 => Ok("q9"),
            Reg::R11 => Ok("q10"),
            Reg::R13 => Ok("q11"),
            _ => invalid_input(format!(
                "AArch64 backend does not map {:?} as FP scratch",
                reg
            )),
        };
    }
    match reg {
        Reg::R10 => Ok("d9"),
        Reg::R11 => Ok("d10"),
        Reg::R13 => Ok("d11"),
        _ => invalid_input(format!(
            "AArch64 backend does not map {:?} as FP scratch",
            reg
        )),
    }
}

fn load_mnemonic(ty: AsmType) -> &'static str {
    match ty {
        AsmType::Byte => "ldrb",
        AsmType::Word => "ldrh",
        AsmType::Longword => "ldr",
        AsmType::Quadword => "ldr",
        AsmType::Octword | AsmType::LongDouble => "ldr",
        AsmType::Float => "ldr",
        AsmType::Double => "ldr",
    }
}

fn store_mnemonic(ty: AsmType) -> &'static str {
    match ty {
        AsmType::Byte => "strb",
        AsmType::Word => "strh",
        AsmType::Longword => "str",
        AsmType::Quadword => "str",
        AsmType::Octword | AsmType::LongDouble => "str",
        AsmType::Float => "str",
        AsmType::Double => "str",
    }
}

fn signed_load_mnemonic(src_ty: AsmType, dst_ty: AsmType) -> io::Result<&'static str> {
    match (src_ty, dst_ty) {
        (AsmType::Byte, AsmType::Word) => Ok("ldrsb"),
        (AsmType::Byte, AsmType::Longword) => Ok("ldrsb"),
        (AsmType::Byte, AsmType::Quadword) => Ok("ldrsb"),
        (AsmType::Word, AsmType::Longword) => Ok("ldrsh"),
        (AsmType::Word, AsmType::Quadword) => Ok("ldrsh"),
        (AsmType::Longword, AsmType::Quadword) => Ok("ldrsw"),
        _ => invalid_input(format!(
            "AArch64 backend does not support sign extension from {:?} to {:?}",
            src_ty, dst_ty
        )),
    }
}

fn condition_name(cc: &CondCode) -> &'static str {
    match cc {
        CondCode::E => "eq",
        CondCode::NE => "ne",
        CondCode::L => "lt",
        CondCode::LE => "le",
        CondCode::G => "gt",
        CondCode::GE => "ge",
        CondCode::A => "hi",
        CondCode::AE => "hs",
        CondCode::B => "lo",
        CondCode::BE => "ls",
        CondCode::P | CondCode::NP => unreachable!("x86 parity condition in AArch64 emitter"),
        CondCode::S | CondCode::NS => unreachable!("x87 sign condition in AArch64 emitter"),
    }
}

fn inverse_condition_name(cc: &CondCode) -> &'static str {
    match cc {
        CondCode::E => "ne",
        CondCode::NE => "eq",
        CondCode::L => "ge",
        CondCode::LE => "gt",
        CondCode::G => "le",
        CondCode::GE => "lt",
        CondCode::A => "ls",
        CondCode::AE => "lo",
        CondCode::B => "hs",
        CondCode::BE => "hi",
        CondCode::P | CondCode::NP => unreachable!("x86 parity condition in AArch64 emitter"),
        CondCode::S | CondCode::NS => unreachable!("x87 sign condition in AArch64 emitter"),
    }
}

fn stack_addr(offset: i32) -> String {
    if offset == 0 {
        "[sp]".to_string()
    } else {
        format!("[sp, #{}]", offset)
    }
}

fn stack_offset_fits_unsigned(ty: AsmType, offset: i32) -> bool {
    if offset < 0 {
        return false;
    }
    match ty {
        AsmType::Byte => offset <= 4095,
        AsmType::Word => offset % 2 == 0 && offset / 2 <= 4095,
        AsmType::Longword => offset % 4 == 0 && offset / 4 <= 4095,
        AsmType::Float => offset % 4 == 0 && offset / 4 <= 4095,
        AsmType::Octword | AsmType::LongDouble => offset % 16 == 0 && offset / 16 <= 4095,
        AsmType::Quadword | AsmType::Double => offset % 8 == 0 && offset / 8 <= 4095,
    }
}

fn emit_stack_address_into(
    w: &mut dyn Write,
    dst_reg: &'static str,
    offset: i32,
) -> std::io::Result<()> {
    if offset == 0 {
        writeln!(w, "\tmov {}, sp", dst_reg)
    } else if (0..=4095).contains(&offset) {
        writeln!(w, "\tadd {}, sp, #{}", dst_reg, offset)
    } else {
        emit_load_immediate(w, AsmType::Quadword, dst_reg, offset as i64)?;
        writeln!(w, "\tadd {}, sp, {}", dst_reg, dst_reg)
    }
}

fn emit_load_stack(
    w: &mut dyn Write,
    ty: AsmType,
    dst_reg: &str,
    offset: i32,
) -> std::io::Result<()> {
    if stack_offset_fits_unsigned(ty, offset) {
        writeln!(
            w,
            "\t{} {}, {}",
            load_mnemonic(ty),
            dst_reg,
            stack_addr(offset)
        )
    } else {
        emit_stack_address_into(w, "x16", offset)?;
        writeln!(w, "\t{} {}, [x16]", load_mnemonic(ty), dst_reg)
    }
}

fn emit_store_stack(
    w: &mut dyn Write,
    ty: AsmType,
    src_reg: &str,
    offset: i32,
) -> std::io::Result<()> {
    if stack_offset_fits_unsigned(ty, offset) {
        writeln!(
            w,
            "\t{} {}, {}",
            store_mnemonic(ty),
            src_reg,
            stack_addr(offset)
        )
    } else {
        emit_stack_address_into(w, "x16", offset)?;
        writeln!(w, "\t{} {}, [x16]", store_mnemonic(ty), src_reg)
    }
}

fn emit_stack_pointer_adjust(w: &mut dyn Write, op: &str, bytes: i64) -> std::io::Result<()> {
    if let Ok(bytes) = i32::try_from(bytes) {
        let offset = match op {
            "add" => bytes,
            "sub" => match bytes.checked_neg() {
                Some(offset) => offset,
                None => return invalid_input("AArch64 stack adjustment is out of range"),
            },
            _ => return invalid_input(format!("unsupported stack adjustment: {op}")),
        };
        return emit_add_immediate(w, "sp", offset);
    }
    emit_load_immediate(w, AsmType::Quadword, "x16", bytes)?;
    writeln!(w, "\t{} sp, sp, x16", op)
}

fn emit_large_stack_pointer_adjust(w: &mut dyn Write, op: &str, bytes: i64) -> std::io::Result<()> {
    if bytes <= 0 {
        return Ok(());
    }
    emit_stack_pointer_adjust(w, op, bytes)
}

fn emit_store_large_local_base(
    w: &mut dyn Write,
    base_offset: i64,
    dst_offset: i32,
) -> std::io::Result<()> {
    if base_offset == 0 {
        writeln!(w, "\tmov x16, sp")?;
    } else if base_offset <= 4095 {
        writeln!(w, "\tadd x16, sp, #{}", base_offset)?;
    } else {
        emit_load_immediate(w, AsmType::Quadword, "x16", base_offset)?;
        writeln!(w, "\tadd x16, sp, x16")?;
    }
    emit_store_stack(w, AsmType::Quadword, "x16", dst_offset)
}

fn data_label(target: &Target, name: &str) -> String {
    target.show_symbol(name)
}

fn offset_data_name(name: &str, add: i32) -> String {
    if let Some(data_offset) = split_data_offset(name) {
        let offset = data_offset.offset + i64::from(add);
        format!("{}{}", data_offset.base, assembly_offset_suffix(offset))
    } else {
        format!("{}{}", name, assembly_offset_suffix(i64::from(add)))
    }
}

fn data_label_expr(target: &Target, name: &str) -> String {
    if let Some(data_offset) = split_data_offset(name) {
        target.show_symbol_with_offset(data_offset.base, data_offset.offset)
    } else {
        data_label(target, name)
    }
}

fn offset_operand(op: &AsmOperand, add: i32) -> std::io::Result<AsmOperand> {
    match op {
        AsmOperand::Stack(offset) => Ok(AsmOperand::Stack(*offset + i64::from(add))),
        AsmOperand::Data(name) => Ok(AsmOperand::Data(offset_data_name(name, add))),
        AsmOperand::Reg(Reg::AX) if add == 8 => Ok(AsmOperand::Reg(Reg::DI)),
        AsmOperand::Reg(reg) if add == 0 => Ok(AsmOperand::Reg(*reg)),
        other => invalid_input(format!(
            "AArch64 backend cannot offset operand {:?} by {}",
            other, add
        )),
    }
}

fn stack_offset_i32(offset: i64) -> std::io::Result<i32> {
    i32::try_from(offset).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("AArch64 stack offset {} is out of range", offset),
        )
    })
}

fn emit_add_immediate(w: &mut dyn Write, reg: &'static str, offset: i32) -> std::io::Result<()> {
    let offset = i64::from(offset);
    let (op, mut remaining) = if offset < 0 {
        ("sub", offset.unsigned_abs())
    } else {
        ("add", offset as u64)
    };
    while remaining >= 4096 {
        let chunk = (remaining >> 12).min(4095);
        writeln!(w, "\t{} {}, {}, #{}, lsl #12", op, reg, reg, chunk)?;
        remaining -= chunk << 12;
    }
    if remaining > 0 {
        writeln!(w, "\t{} {}, {}, #{}", op, reg, reg, remaining)?;
    }
    Ok(())
}

fn emit_load_macho_data_offset_address(
    w: &mut dyn Write,
    target: &Target,
    name: &str,
    addr_reg: &'static str,
) -> std::io::Result<()> {
    if let Some(data_offset) = split_data_offset(name) {
        let base_label = data_label(target, data_offset.base);
        writeln!(w, "\tadrp {}, {}@PAGE", addr_reg, base_label)?;
        writeln!(
            w,
            "\tadd {}, {}, {}@PAGEOFF",
            addr_reg, addr_reg, base_label
        )?;
        if data_offset.offset != 0 {
            emit_add_immediate(w, addr_reg, data_offset_i32(data_offset.offset)?)?;
        }
        Ok(())
    } else {
        let label = data_label(target, name);
        writeln!(w, "\tadrp {}, {}@PAGE", addr_reg, label)?;
        writeln!(w, "\tadd {}, {}, {}@PAGEOFF", addr_reg, addr_reg, label)
    }
}

fn data_offset_i32(offset: i64) -> std::io::Result<i32> {
    i32::try_from(offset).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("AArch64 data offset {} is out of range", offset),
        )
    })
}

fn emit_load_data_address(
    w: &mut dyn Write,
    target: &Target,
    name: &str,
    dst_reg: &'static str,
) -> std::io::Result<()> {
    let label = data_label(target, name);
    match target.os {
        TargetOs::Linux => {
            writeln!(w, "\tadrp {}, {}", dst_reg, label)?;
            writeln!(w, "\tadd {}, {}, :lo12:{}", dst_reg, dst_reg, label)
        }
        TargetOs::MacOs => {
            writeln!(w, "\tadrp {}, {}@GOTPAGE", dst_reg, label)?;
            writeln!(w, "\tldr {}, [{}, {}@GOTPAGEOFF]", dst_reg, dst_reg, label)
        }
    }
}

fn emit_load_tls_address(
    w: &mut dyn Write,
    target: &Target,
    name: &str,
    offset: i32,
    dst_reg: &'static str,
) -> std::io::Result<()> {
    let label = data_label(target, name);
    match target.os {
        TargetOs::Linux => {
            writeln!(w, "\tmrs {}, tpidr_el0", dst_reg)?;
            writeln!(w, "\tadd {}, {}, :tprel_hi12:{}", dst_reg, dst_reg, label)?;
            writeln!(
                w,
                "\tadd {}, {}, :tprel_lo12_nc:{}",
                dst_reg, dst_reg, label
            )?;
            if offset != 0 {
                emit_load_immediate(w, AsmType::Quadword, "x17", offset as i64)?;
                writeln!(w, "\tadd {}, {}, x17", dst_reg, dst_reg)?;
            }
            Ok(())
        }
        TargetOs::MacOs => {
            writeln!(w, "\tadrp x0, {}@TLVPPAGE", label)?;
            writeln!(w, "\tldr x0, [x0, {}@TLVPPAGEOFF]", label)?;
            writeln!(w, "\tldr x8, [x0]")?;
            writeln!(w, "\tstp x9, x10, [sp, #-64]!")?;
            writeln!(w, "\tstp x11, x12, [sp, #16]")?;
            writeln!(w, "\tstp x13, x14, [sp, #32]")?;
            writeln!(w, "\tstp x15, x30, [sp, #48]")?;
            writeln!(w, "\tblr x8")?;
            writeln!(w, "\tldp x11, x12, [sp, #16]")?;
            writeln!(w, "\tldp x13, x14, [sp, #32]")?;
            writeln!(w, "\tldp x15, x30, [sp, #48]")?;
            writeln!(w, "\tldp x9, x10, [sp], #64")?;
            if dst_reg != "x0" {
                writeln!(w, "\tmov {}, x0", dst_reg)?;
            }
            if offset != 0 {
                emit_load_immediate(w, AsmType::Quadword, "x17", offset as i64)?;
                writeln!(w, "\tadd {}, {}, x17", dst_reg, dst_reg)?;
            }
            Ok(())
        }
    }
}

fn emit_load_tls_data(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    name: &str,
    offset: i32,
    dst_reg: &'static str,
) -> std::io::Result<()> {
    emit_load_tls_address(w, target, name, offset, "x16")?;
    writeln!(w, "\t{} {}, [x16]", load_mnemonic(ty), dst_reg)
}

fn exclusive_load_mnemonic(ty: AsmType) -> io::Result<&'static str> {
    match ty {
        AsmType::Byte => Ok("ldaxrb"),
        AsmType::Word => Ok("ldaxrh"),
        AsmType::Longword | AsmType::Quadword => Ok("ldaxr"),
        _ => invalid_input(format!("unsupported atomic load type: {:?}", ty)),
    }
}

fn exclusive_store_mnemonic(ty: AsmType) -> io::Result<&'static str> {
    match ty {
        AsmType::Byte => Ok("stlxrb"),
        AsmType::Word => Ok("stlxrh"),
        AsmType::Longword | AsmType::Quadword => Ok("stlxr"),
        _ => invalid_input(format!("unsupported atomic store type: {:?}", ty)),
    }
}

fn emit_atomic_rmw(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    op: &AsmBinaryOp,
    return_old: bool,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let load = exclusive_load_mnemonic(ty)?;
    let store = exclusive_store_mnemonic(ty)?;
    let old = if ty == AsmType::Quadword {
        "x12"
    } else {
        "w12"
    };
    let arg = reg_name(Reg::R10, ty)?;
    let new = if ty == AsmType::Quadword {
        "x14"
    } else {
        "w14"
    };
    let status = "w13";
    let ptr = "x10";
    writeln!(w, "1:")?;
    writeln!(w, "\t{} {}, [{}]", load, old, ptr)?;
    match op {
        AsmBinaryOp::Add => writeln!(w, "\tadd {}, {}, {}", new, old, arg)?,
        AsmBinaryOp::Sub => writeln!(w, "\tsub {}, {}, {}", new, old, arg)?,
        AsmBinaryOp::And => writeln!(w, "\tand {}, {}, {}", new, old, arg)?,
        AsmBinaryOp::Nand => {
            writeln!(w, "\tand {}, {}, {}", new, old, arg)?;
            writeln!(w, "\tmvn {}, {}", new, new)?
        }
        AsmBinaryOp::Or => writeln!(w, "\torr {}, {}, {}", new, old, arg)?,
        AsmBinaryOp::Xor => writeln!(w, "\teor {}, {}, {}", new, old, arg)?,
        _ => return invalid_input(format!("unsupported atomic rmw op: {:?}", op)),
    }
    writeln!(w, "\t{} {}, {}, [{}]", store, status, new, ptr)?;
    writeln!(w, "\tcbnz {}, 1b", status)?;
    store_operand(w, target, ty, if return_old { old } else { new }, dst)
}

fn emit_atomic_exchange(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let load = exclusive_load_mnemonic(ty)?;
    let store = exclusive_store_mnemonic(ty)?;
    let old = if ty == AsmType::Quadword {
        "x12"
    } else {
        "w12"
    };
    let value = reg_name(Reg::R10, ty)?;
    let status = "w13";
    let ptr = "x10";
    writeln!(w, "1:")?;
    writeln!(w, "\t{} {}, [{}]", load, old, ptr)?;
    writeln!(w, "\t{} {}, {}, [{}]", store, status, value, ptr)?;
    writeln!(w, "\tcbnz {}, 1b", status)?;
    store_operand(w, target, ty, old, dst)
}

fn emit_atomic_compare_exchange(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let load = exclusive_load_mnemonic(ty)?;
    let store_exclusive = exclusive_store_mnemonic(ty)?;
    let load_expected = load_mnemonic(ty);
    let store_expected = store_mnemonic(ty);
    let old = if ty == AsmType::Quadword {
        "x12"
    } else {
        "w12"
    };
    let expected = if ty == AsmType::Quadword {
        "x14"
    } else {
        "w14"
    };
    let desired = reg_name(Reg::R10, ty)?;
    let status = "w13";
    let ptr = "x10";
    let expected_ptr = "x7";
    writeln!(w, "\t{} {}, [{}]", load_expected, expected, expected_ptr)?;
    writeln!(w, "1:")?;
    writeln!(w, "\t{} {}, [{}]", load, old, ptr)?;
    writeln!(w, "\tcmp {}, {}", old, expected)?;
    writeln!(w, "\tb.ne 2f")?;
    writeln!(
        w,
        "\t{} {}, {}, [{}]",
        store_exclusive, status, desired, ptr
    )?;
    writeln!(w, "\tcbnz {}, 1b", status)?;
    writeln!(w, "\tmov w15, #1")?;
    writeln!(w, "\tb 3f")?;
    writeln!(w, "2:")?;
    writeln!(w, "\tclrex")?;
    writeln!(w, "\t{} {}, [{}]", store_expected, old, expected_ptr)?;
    writeln!(w, "\tmov w15, wzr")?;
    writeln!(w, "3:")?;
    store_operand(w, target, AsmType::Byte, "w15", dst)
}

fn emit_atomic_compare_swap(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    return_old: bool,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let load = exclusive_load_mnemonic(ty)?;
    let store = exclusive_store_mnemonic(ty)?;
    let old = if ty == AsmType::Quadword {
        "x12"
    } else {
        "w12"
    };
    let expected = reg_name(Reg::R12, ty)?;
    let desired = reg_name(Reg::R10, ty)?;
    let status = "w13";
    let ptr = "x10";
    writeln!(w, "1:")?;
    writeln!(w, "\t{} {}, [{}]", load, old, ptr)?;
    writeln!(w, "\tcmp {}, {}", old, expected)?;
    writeln!(w, "\tb.ne 2f")?;
    writeln!(w, "\t{} {}, {}, [{}]", store, status, desired, ptr)?;
    writeln!(w, "\tcbnz {}, 1b", status)?;
    if !return_old {
        writeln!(w, "\tmov w15, #1")?;
        writeln!(w, "\tb 3f")?;
    }
    writeln!(w, "2:")?;
    writeln!(w, "\tclrex")?;
    if return_old {
        return store_operand(w, target, ty, old, dst);
    }
    writeln!(w, "\tmov w15, wzr")?;
    writeln!(w, "3:")?;
    store_operand(w, target, AsmType::Byte, "w15", dst)
}

fn emit_store_tls_data(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src_reg: &'static str,
    name: &str,
    offset: i32,
) -> std::io::Result<()> {
    if target.os == TargetOs::MacOs {
        writeln!(w, "\tsub sp, sp, #16")?;
        writeln!(w, "\t{} {}, [sp]", store_mnemonic(ty), src_reg)?;
        emit_load_tls_address(w, target, name, offset, "x16")?;
        let scratch = match ty {
            AsmType::Float => "s16",
            AsmType::Double => "d16",
            AsmType::Byte | AsmType::Word | AsmType::Longword => "w17",
            AsmType::Quadword => "x17",
            AsmType::Octword | AsmType::LongDouble => {
                return invalid_input("AArch64 emitter needs 128-bit TLS lowering")
            }
        };
        writeln!(w, "\t{} {}, [sp]", load_mnemonic(ty), scratch)?;
        writeln!(w, "\t{} {}, [x16]", store_mnemonic(ty), scratch)?;
        return writeln!(w, "\tadd sp, sp, #16");
    }
    emit_load_tls_address(w, target, name, offset, "x16")?;
    writeln!(w, "\t{} {}, [x16]", store_mnemonic(ty), src_reg)
}

fn emit_load_data(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    name: &str,
    dst_reg: &'static str,
) -> std::io::Result<()> {
    let label = data_label(target, name);
    let addr_reg = "x16";
    match target.os {
        TargetOs::Linux => {
            let label = data_label_expr(target, name);
            writeln!(w, "\tadrp {}, {}", addr_reg, label)?;
            writeln!(
                w,
                "\t{} {}, [{}, :lo12:{}]",
                load_mnemonic(ty),
                dst_reg,
                addr_reg,
                label
            )
        }
        TargetOs::MacOs => {
            if name.contains('+') {
                emit_load_macho_data_offset_address(w, target, name, addr_reg)?;
                writeln!(w, "\t{} {}, [{}]", load_mnemonic(ty), dst_reg, addr_reg)
            } else {
                writeln!(w, "\tadrp {}, {}@GOTPAGE", addr_reg, label)?;
                writeln!(
                    w,
                    "\tldr {}, [{}, {}@GOTPAGEOFF]",
                    addr_reg, addr_reg, label
                )?;
                writeln!(w, "\t{} {}, [{}]", load_mnemonic(ty), dst_reg, addr_reg)
            }
        }
    }
}

fn emit_store_data(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src_reg: &'static str,
    name: &str,
) -> std::io::Result<()> {
    let label = data_label(target, name);
    let addr_reg = "x16";
    match target.os {
        TargetOs::Linux => {
            let label = data_label_expr(target, name);
            writeln!(w, "\tadrp {}, {}", addr_reg, label)?;
            writeln!(
                w,
                "\t{} {}, [{}, :lo12:{}]",
                store_mnemonic(ty),
                src_reg,
                addr_reg,
                label
            )
        }
        TargetOs::MacOs => {
            if name.contains('+') {
                emit_load_macho_data_offset_address(w, target, name, addr_reg)?;
                writeln!(w, "\t{} {}, [{}]", store_mnemonic(ty), src_reg, addr_reg)
            } else {
                writeln!(w, "\tadrp {}, {}@GOTPAGE", addr_reg, label)?;
                writeln!(
                    w,
                    "\tldr {}, [{}, {}@GOTPAGEOFF]",
                    addr_reg, addr_reg, label
                )?;
                writeln!(w, "\t{} {}, [{}]", store_mnemonic(ty), src_reg, addr_reg)
            }
        }
    }
}

fn emit_load_data_extended(
    w: &mut dyn Write,
    target: &Target,
    src_ty: AsmType,
    dst_ty: AsmType,
    name: &str,
    dst_reg: &'static str,
) -> std::io::Result<()> {
    let label = data_label(target, name);
    let mnemonic = signed_load_mnemonic(src_ty, dst_ty)?;
    let addr_reg = "x16";
    match target.os {
        TargetOs::Linux => {
            let label = data_label_expr(target, name);
            writeln!(w, "\tadrp {}, {}", addr_reg, label)?;
            writeln!(
                w,
                "\t{} {}, [{}, :lo12:{}]",
                mnemonic, dst_reg, addr_reg, label
            )
        }
        TargetOs::MacOs => {
            if name.contains('+') {
                emit_load_macho_data_offset_address(w, target, name, addr_reg)?;
                writeln!(w, "\t{} {}, [{}]", mnemonic, dst_reg, addr_reg)
            } else {
                writeln!(w, "\tadrp {}, {}@GOTPAGE", addr_reg, label)?;
                writeln!(
                    w,
                    "\tldr {}, [{}, {}@GOTPAGEOFF]",
                    addr_reg, addr_reg, label
                )?;
                writeln!(w, "\t{} {}, [{}]", mnemonic, dst_reg, addr_reg)
            }
        }
    }
}

fn emit_load_immediate(
    w: &mut dyn Write,
    ty: AsmType,
    reg: &'static str,
    value: i64,
) -> std::io::Result<()> {
    if value == 0 {
        let zero_reg = zero_register_for_type(ty)?;
        return writeln!(w, "\tmov {}, {}", reg, zero_reg);
    }

    let width = match ty {
        AsmType::Byte | AsmType::Word | AsmType::Longword => 32,
        AsmType::Quadword => 64,
        AsmType::Octword | AsmType::LongDouble => {
            return invalid_input("AArch64 emitter needs 128-bit immediates")
        }
        AsmType::Float => 32,
        AsmType::Double => 64,
    };
    let bits = if width == 32 {
        value as u32 as u64
    } else {
        value as u64
    };

    let chunks: Vec<_> = (0..width)
        .step_by(16)
        .map(|shift| ((bits >> shift) & 0xffff) as u16)
        .collect();
    let zero_cost = chunks.iter().filter(|&&chunk| chunk != 0).count();
    let ones_cost = chunks.iter().filter(|&&chunk| chunk != u16::MAX).count();
    let use_movn = ones_cost < zero_cost;
    let base = if use_movn {
        chunks
            .iter()
            .position(|&chunk| chunk != u16::MAX)
            .unwrap_or(0)
    } else {
        chunks
            .iter()
            .position(|&chunk| chunk != 0)
            .expect("nonzero immediate must have a nonzero chunk")
    };
    let base_shift = base * 16;
    let base_chunk = chunks[base];
    let (base_op, base_value) = if use_movn {
        ("movn", !base_chunk)
    } else {
        ("movz", base_chunk)
    };
    if base_shift == 0 {
        writeln!(w, "\t{} {}, #{}", base_op, reg, base_value)?;
    } else {
        writeln!(
            w,
            "\t{} {}, #{}, lsl #{}",
            base_op, reg, base_value, base_shift
        )?;
    }
    for (index, &chunk) in chunks.iter().enumerate() {
        if index == base || chunk == if use_movn { u16::MAX } else { 0 } {
            continue;
        }
        let shift = index * 16;
        if shift == 0 {
            writeln!(w, "\tmovk {}, #{}", reg, chunk)?;
        } else {
            writeln!(w, "\tmovk {}, #{}, lsl #{}", reg, chunk, shift)?;
        }
    }

    Ok(())
}

fn zero_register_for_type(ty: AsmType) -> std::io::Result<&'static str> {
    match ty {
        AsmType::Byte | AsmType::Word | AsmType::Longword | AsmType::Float => Ok("wzr"),
        AsmType::Quadword | AsmType::Double => Ok("xzr"),
        AsmType::Octword | AsmType::LongDouble => {
            invalid_input("AArch64 emitter needs 128-bit zero register handling")
        }
    }
}

/// Return an AArch64 `fmov` immediate spelling for an exactly encodable IEEE
/// value. The immediate format represents `(1 + fraction / 16) * 2^exponent`
/// for exponents from -3 through 4, with either sign. The IR stores raw IEEE
/// bits, so this recognizes the format without depending on host parsing.
fn aarch64_fmov_immediate(ty: AsmType, value: i64) -> Option<String> {
    let (negative, exponent, fraction) = match ty {
        AsmType::Float => {
            let bits = value as u32;
            let exponent = ((bits >> 23) & 0xff) as i32 - 127;
            if bits & 0x0007_ffff != 0 {
                return None;
            }
            (bits >> 31 != 0, exponent, (bits & 0x7f_ffff) >> 19)
        }
        AsmType::Double => {
            let bits = value as u64;
            let exponent = ((bits >> 52) & 0x7ff) as i32 - 1023;
            if bits & 0x0000_ffff_ffff_ffff != 0 {
                return None;
            }
            (bits >> 63 != 0, exponent, ((bits >> 48) & 0xf) as u32)
        }
        _ => return None,
    };
    if !(-3..=4).contains(&exponent) {
        return None;
    }
    let magnitude = (16 + fraction) as f64 * 2f64.powi(exponent - 4);
    let value = if negative { -magnitude } else { magnitude };
    Some(if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    })
}

fn load_operand(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    operand: &AsmOperand,
    scratch: Reg,
) -> std::io::Result<&'static str> {
    let reg = if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
        fp_scratch_name_typed(scratch, ty)?
    } else {
        reg_name(scratch, ty)?
    };
    match operand {
        AsmOperand::Imm(value) => {
            if matches!(ty, AsmType::Float | AsmType::Double) {
                if *value == 0 {
                    let zero_reg = zero_register_for_type(ty)?;
                    writeln!(w, "\tfmov {}, {}", reg, zero_reg)?;
                } else if let Some(immediate) = aarch64_fmov_immediate(ty, *value) {
                    writeln!(w, "\tfmov {}, #{}", reg, immediate)?;
                } else {
                    let int_ty = if ty == AsmType::Float {
                        AsmType::Longword
                    } else {
                        AsmType::Quadword
                    };
                    let int_reg = reg_name(scratch, int_ty)?;
                    emit_load_immediate(w, int_ty, int_reg, *value)?;
                    writeln!(w, "\tfmov {}, {}", reg, int_reg)?;
                }
            } else {
                if *value == 0 {
                    let zero_reg = if ty == AsmType::Quadword {
                        "xzr"
                    } else {
                        "wzr"
                    };
                    writeln!(w, "\tmov {}, {}", reg, zero_reg)?;
                } else {
                    emit_load_immediate(w, ty, reg, *value)?;
                }
            }
        }
        AsmOperand::Reg(reg) => return reg_name(*reg, ty),
        AsmOperand::Xmm(reg) => return fp_name_typed(*reg, ty),
        AsmOperand::Stack(offset) => {
            emit_load_stack(w, ty, reg, stack_offset_i32(*offset)?)?;
        }
        AsmOperand::Data(name) => emit_load_data(w, target, ty, name, reg)?,
        AsmOperand::TlsData(name, offset) => emit_load_tls_data(w, target, ty, name, *offset, reg)?,
        other => {
            return invalid_input(format!(
                "AArch64 backend cannot load operand yet: {:?}",
                other
            ))
        }
    }
    Ok(reg)
}

fn store_operand(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src_reg: &'static str,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    match dst {
        AsmOperand::Reg(reg) => {
            let dst_reg = reg_name(*reg, ty)?;
            if dst_reg == src_reg {
                Ok(())
            } else {
                writeln!(w, "\tmov {}, {}", dst_reg, src_reg)
            }
        }
        AsmOperand::Xmm(reg) if ty == AsmType::LongDouble => {
            if fp_name_typed(*reg, ty)? == src_reg {
                Ok(())
            } else {
                writeln!(
                    w,
                    "\tmov {}.16b, {}",
                    fp_vector_name(*reg),
                    q_reg_to_vector_name(src_reg)?
                )
            }
        }
        AsmOperand::Xmm(reg) => {
            let dst_reg = fp_name_typed(*reg, ty)?;
            if dst_reg == src_reg {
                Ok(())
            } else {
                writeln!(w, "\tfmov {}, {}", dst_reg, src_reg)
            }
        }
        AsmOperand::Stack(offset) => emit_store_stack(w, ty, src_reg, stack_offset_i32(*offset)?),
        AsmOperand::Data(name) => emit_store_data(w, target, ty, src_reg, name),
        AsmOperand::TlsData(name, offset) => {
            emit_store_tls_data(w, target, ty, src_reg, name, *offset)
        }
        other => invalid_input(format!(
            "AArch64 backend cannot store operand yet: {:?}",
            other
        )),
    }
}

fn emit_mov(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    if ty == AsmType::Octword {
        let src_low = offset_operand(src, 0)?;
        let src_high = offset_operand(src, 8)?;
        let dst_low = offset_operand(dst, 0)?;
        let dst_high = offset_operand(dst, 8)?;
        emit_mov(w, target, AsmType::Quadword, &src_low, &dst_low)?;
        return emit_mov(w, target, AsmType::Quadword, &src_high, &dst_high);
    }
    match (src, dst) {
        (AsmOperand::Imm(value), AsmOperand::Xmm(reg))
            if matches!(ty, AsmType::Float | AsmType::Double) =>
        {
            if *value == 0 {
                let zero_reg = if ty == AsmType::Float { "wzr" } else { "xzr" };
                writeln!(w, "\tfmov {}, {}", fp_name_typed(*reg, ty)?, zero_reg)
            } else if let Some(immediate) = aarch64_fmov_immediate(ty, *value) {
                writeln!(w, "\tfmov {}, #{}", fp_name_typed(*reg, ty)?, immediate)
            } else {
                let int_ty = if ty == AsmType::Float {
                    AsmType::Longword
                } else {
                    AsmType::Quadword
                };
                let int_reg = if ty == AsmType::Float { "w9" } else { "x9" };
                emit_load_immediate(w, int_ty, int_reg, *value)?;
                writeln!(w, "\tfmov {}, {}", fp_name_typed(*reg, ty)?, int_reg)
            }
        }
        (AsmOperand::Imm(value), AsmOperand::Reg(reg)) => {
            emit_load_immediate(w, ty, reg_name(*reg, ty)?, *value)
        }
        (AsmOperand::Xmm(src), AsmOperand::Xmm(dst)) if ty == AsmType::LongDouble => {
            if src == dst {
                Ok(())
            } else {
                writeln!(
                    w,
                    "\tmov {}.16b, {}.16b",
                    fp_vector_name(*dst),
                    fp_vector_name(*src)
                )
            }
        }
        (AsmOperand::Xmm(src), AsmOperand::Xmm(dst))
            if matches!(ty, AsmType::Float | AsmType::Double) =>
        {
            if src == dst {
                Ok(())
            } else {
                writeln!(
                    w,
                    "\tfmov {}, {}",
                    fp_name_typed(*dst, ty)?,
                    fp_name_typed(*src, ty)?
                )
            }
        }
        (AsmOperand::Stack(offset), AsmOperand::Xmm(reg))
            if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) =>
        {
            emit_load_stack(w, ty, fp_name_typed(*reg, ty)?, stack_offset_i32(*offset)?)
        }
        (AsmOperand::Xmm(reg), AsmOperand::Stack(offset))
            if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) =>
        {
            emit_store_stack(w, ty, fp_name_typed(*reg, ty)?, stack_offset_i32(*offset)?)
        }
        (AsmOperand::Data(name), AsmOperand::Xmm(reg))
            if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) =>
        {
            emit_load_data(w, target, ty, name, fp_name_typed(*reg, ty)?)
        }
        (AsmOperand::TlsData(name, offset), AsmOperand::Xmm(reg))
            if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) =>
        {
            emit_load_tls_data(w, target, ty, name, *offset, fp_name_typed(*reg, ty)?)
        }
        (AsmOperand::Xmm(reg), AsmOperand::Data(name))
            if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) =>
        {
            emit_store_data(w, target, ty, fp_name_typed(*reg, ty)?, name)
        }
        (AsmOperand::Xmm(reg), AsmOperand::TlsData(name, offset))
            if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) =>
        {
            emit_store_tls_data(w, target, ty, fp_name_typed(*reg, ty)?, name, *offset)
        }
        (AsmOperand::Reg(src), AsmOperand::Reg(dst)) if src == dst => Ok(()),
        (AsmOperand::Reg(src), AsmOperand::Reg(dst)) => {
            writeln!(w, "\tmov {}, {}", reg_name(*dst, ty)?, reg_name(*src, ty)?)
        }
        (AsmOperand::Stack(offset), AsmOperand::Reg(reg)) => {
            emit_load_stack(w, ty, reg_name(*reg, ty)?, stack_offset_i32(*offset)?)
        }
        (AsmOperand::Reg(reg), AsmOperand::Stack(offset)) => {
            emit_store_stack(w, ty, reg_name(*reg, ty)?, stack_offset_i32(*offset)?)
        }
        (AsmOperand::Data(name), AsmOperand::Reg(reg)) => {
            emit_load_data(w, target, ty, name, reg_name(*reg, ty)?)
        }
        (AsmOperand::Reg(reg), AsmOperand::Data(name)) => {
            emit_store_data(w, target, ty, reg_name(*reg, ty)?, name)
        }
        _ => {
            let scratch = load_operand(w, target, ty, src, Reg::R10)?;
            store_operand(w, target, ty, scratch, dst)
        }
    }
}

fn emit_movsx(
    w: &mut dyn Write,
    target: &Target,
    src_ty: AsmType,
    dst_ty: AsmType,
    src: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    if src_ty == dst_ty {
        return emit_mov(w, target, dst_ty, src, dst);
    }
    if src_ty == AsmType::Octword {
        let src_low = offset_operand(src, 0)?;
        return emit_mov(w, target, dst_ty, &src_low, dst);
    }
    let dst_reg = reg_name(Reg::R10, dst_ty)?;
    match src {
        AsmOperand::Stack(offset) => {
            let mnemonic = signed_load_mnemonic(src_ty, dst_ty)?;
            let offset = stack_offset_i32(*offset)?;
            if stack_offset_fits_unsigned(src_ty, offset) {
                writeln!(w, "\t{} {}, {}", mnemonic, dst_reg, stack_addr(offset))?;
            } else {
                emit_stack_address_into(w, "x16", offset)?;
                writeln!(w, "\t{} {}, [x16]", mnemonic, dst_reg)?;
            }
        }
        AsmOperand::Data(name) => {
            emit_load_data_extended(w, target, src_ty, dst_ty, name, dst_reg)?;
        }
        AsmOperand::TlsData(name, offset) => {
            emit_load_tls_address(w, target, name, *offset, "x16")?;
            writeln!(
                w,
                "\t{} {}, [x16]",
                signed_load_mnemonic(src_ty, dst_ty)?,
                dst_reg
            )?;
        }
        AsmOperand::Reg(reg) => {
            let src_reg = reg_name(*reg, src_ty)?;
            let mnemonic = match (src_ty, dst_ty) {
                (AsmType::Byte, AsmType::Word)
                | (AsmType::Byte, AsmType::Longword)
                | (AsmType::Byte, AsmType::Quadword) => "sxtb",
                (AsmType::Word, AsmType::Longword) | (AsmType::Word, AsmType::Quadword) => "sxth",
                (AsmType::Longword, AsmType::Quadword) => "sxtw",
                _ => {
                    return invalid_input(format!(
                        "AArch64 backend does not support sign extension from {:?} to {:?}",
                        src_ty, dst_ty
                    ))
                }
            };
            writeln!(w, "\t{} {}, {}", mnemonic, dst_reg, src_reg)?;
        }
        AsmOperand::Imm(value) => {
            emit_load_immediate(w, dst_ty, dst_reg, *value)?;
        }
        other => {
            return invalid_input(format!(
                "AArch64 backend cannot sign-extend operand yet: {:?}",
                other
            ))
        }
    }
    store_operand(w, target, dst_ty, dst_reg, dst)
}

fn emit_mov_zero_extend(
    w: &mut dyn Write,
    target: &Target,
    src_ty: AsmType,
    dst_ty: AsmType,
    src: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    fn int_size(ty: AsmType) -> Option<usize> {
        match ty {
            AsmType::Byte => Some(1),
            AsmType::Word => Some(2),
            AsmType::Longword => Some(4),
            AsmType::Quadword => Some(8),
            AsmType::Octword | AsmType::LongDouble => Some(16),
            AsmType::Float | AsmType::Double => None,
        }
    }

    if int_size(src_ty)
        .zip(int_size(dst_ty))
        .is_some_and(|(src, dst)| src >= dst)
        && !matches!(dst_ty, AsmType::Octword | AsmType::LongDouble)
    {
        return emit_mov(w, target, dst_ty, src, dst);
    }

    if matches!(dst_ty, AsmType::Octword | AsmType::LongDouble) {
        if matches!(src_ty, AsmType::Octword | AsmType::LongDouble) {
            return emit_mov(w, target, dst_ty, src, dst);
        }
        let src_reg = load_operand(w, target, src_ty, src, Reg::R10)?;
        match src_ty {
            AsmType::Byte => writeln!(w, "\tand w9, {}, #255", src_reg)?,
            AsmType::Word => writeln!(w, "\tand w9, {}, #65535", src_reg)?,
            AsmType::Longword => {
                if src_reg != "w9" {
                    writeln!(w, "\tmov w9, {}", src_reg)?;
                }
            }
            AsmType::Quadword => {
                if src_reg != "x9" {
                    writeln!(w, "\tmov x9, {}", src_reg)?;
                }
            }
            AsmType::Octword | AsmType::LongDouble => {
                return invalid_input("AArch64 emitter does not support 128-bit zero extension")
            }
            AsmType::Float | AsmType::Double => {
                return invalid_input("AArch64 backend does not support float zero extension")
            }
        }
        let dst_low = offset_operand(dst, 0)?;
        let dst_high = offset_operand(dst, 8)?;
        store_operand(w, target, AsmType::Quadword, "x9", &dst_low)?;
        return store_operand(w, target, AsmType::Quadword, "xzr", &dst_high);
    }
    if matches!(src_ty, AsmType::Octword | AsmType::LongDouble) {
        let src_low = offset_operand(src, 0)?;
        return emit_mov(w, target, dst_ty, &src_low, dst);
    }

    if let AsmOperand::Reg(reg) = src {
        let src_reg = reg_name(*reg, src_ty)?;
        match src_ty {
            AsmType::Byte => {
                writeln!(w, "\tand w9, {}, #255", src_reg)?;
            }
            AsmType::Word => {
                writeln!(w, "\tand w9, {}, #65535", src_reg)?;
            }
            AsmType::Longword if dst_ty == AsmType::Quadword => {
                writeln!(w, "\tmov w9, {}", src_reg)?;
            }
            _ => {
                writeln!(w, "\tmov {}, {}", reg_name(Reg::R10, dst_ty)?, src_reg)?;
            }
        }
        return store_operand(w, target, dst_ty, reg_name(Reg::R10, dst_ty)?, dst);
    }

    let src_reg = load_operand(w, target, src_ty, src, Reg::R10)?;
    let store_reg = match dst_ty {
        AsmType::Byte | AsmType::Word | AsmType::Longword => reg_name(Reg::R10, AsmType::Longword)?,
        AsmType::Quadword => reg_name(Reg::R10, AsmType::Quadword)?,
        AsmType::Octword | AsmType::LongDouble => {
            return invalid_input("AArch64 emitter does not support 128-bit zero extension")
        }
        AsmType::Float | AsmType::Double => {
            return invalid_input("AArch64 backend does not support double zero extension")
        }
    };
    if src_reg != store_reg && dst_ty != AsmType::Quadword {
        writeln!(w, "\tmov {}, {}", store_reg, src_reg)?;
    }
    store_operand(w, target, dst_ty, store_reg, dst)
}

fn emit_unary(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    op: &AsmUnaryOp,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let reg = load_operand(w, target, ty, dst, Reg::R10)?;
    if matches!(ty, AsmType::Float | AsmType::Double) {
        let mnemonic = match op {
            AsmUnaryOp::Neg => "fneg",
            AsmUnaryOp::Not => {
                return invalid_input(
                    "AArch64 backend does not support bitwise-not on double values",
                )
            }
        };
        writeln!(w, "\t{} {}, {}", mnemonic, reg, reg)?;
        return store_operand(w, target, ty, reg, dst);
    }
    let mnemonic = match op {
        AsmUnaryOp::Neg => "neg",
        AsmUnaryOp::Not => "mvn",
    };
    writeln!(w, "\t{} {}, {}", mnemonic, reg, reg)?;
    store_operand(w, target, ty, reg, dst)
}

fn emit_binary(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    op: &AsmBinaryOp,
    src: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    if matches!(ty, AsmType::Float | AsmType::Double) {
        let dst_reg = load_operand(w, target, ty, dst, Reg::R10)?;
        let src_reg = load_operand(w, target, ty, src, Reg::R11)?;
        let mnemonic = match op {
            AsmBinaryOp::Add => "fadd",
            AsmBinaryOp::Sub => "fsub",
            AsmBinaryOp::Mul => "fmul",
            AsmBinaryOp::DivDouble => "fdiv",
            other => {
                return invalid_input(format!(
                    "AArch64 backend does not support floating binary op yet: {:?}",
                    other
                ))
            }
        };
        writeln!(w, "\t{} {}, {}, {}", mnemonic, dst_reg, dst_reg, src_reg)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }

    if let Some(value) = binary_mul_trivial_value(ty, op, src) {
        return match value {
            0 => store_operand(w, target, ty, zero_register_for_type(ty)?, dst),
            1 => Ok(()),
            _ => unreachable!("only zero and one are trivial multiplication values"),
        };
    }
    if binary_divide_by_one(ty, op, src) {
        return Ok(());
    }
    if binary_logical_noop(ty, op, src) {
        return Ok(());
    }
    if binary_logical_zero_result(ty, op, src) {
        return store_operand(w, target, ty, zero_register_for_type(ty)?, dst);
    }
    if binary_logical_all_ones_result(ty, op, src) {
        return emit_mov(w, target, ty, &AsmOperand::Imm(-1), dst);
    }
    let dst_reg = load_operand(w, target, ty, dst, Reg::R10)?;
    if let Some(offset) = binary_add_sub_immediate_offset(op, src) {
        if matches!(op, AsmBinaryOp::AddSetFlags | AsmBinaryOp::SubSetFlags) {
            emit_add_set_flags_immediate(w, dst_reg, offset)?;
            return store_operand(w, target, ty, dst_reg, dst);
        }
        emit_add_immediate(w, dst_reg, offset)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    let mnemonic = match op {
        AsmBinaryOp::Add => "add",
        AsmBinaryOp::AddSetFlags => "adds",
        AsmBinaryOp::Adc => "adcs",
        AsmBinaryOp::Sub => "sub",
        AsmBinaryOp::SubSetFlags => "subs",
        AsmBinaryOp::Sbb => "sbcs",
        AsmBinaryOp::Mul => "mul",
        AsmBinaryOp::SDiv => "sdiv",
        AsmBinaryOp::UDiv => "udiv",
        AsmBinaryOp::And => "and",
        AsmBinaryOp::Or => "orr",
        AsmBinaryOp::Xor => "eor",
        AsmBinaryOp::Sal => "lsl",
        AsmBinaryOp::Sar => "asr",
        AsmBinaryOp::Shr => "lsr",
        other => {
            return invalid_input(format!(
                "AArch64 backend does not support binary op yet: {:?}",
                other
            ))
        }
    };
    if let Some(amount) = binary_shift_immediate_amount(ty, op, src) {
        writeln!(w, "\t{} {}, {}, #{}", mnemonic, dst_reg, dst_reg, amount)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if let Some(amount) = binary_mul_power_of_two_amount(ty, op, src) {
        writeln!(w, "\tlsl {}, {}, #{}", dst_reg, dst_reg, amount)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if let Some(amount) = binary_mul_negative_power_of_two_amount(ty, op, src) {
        writeln!(w, "\tlsl {}, {}, #{}", dst_reg, dst_reg, amount)?;
        writeln!(w, "\tneg {}, {}", dst_reg, dst_reg)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if binary_mul_negative_one(ty, op, src) {
        writeln!(w, "\tneg {}, {}", dst_reg, dst_reg)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if binary_signed_divide_by_negative_one(ty, op, src) {
        writeln!(w, "\tneg {}, {}", dst_reg, dst_reg)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if let Some((amount, width, negate)) = binary_signed_div_power_of_two_amount(ty, op, src) {
        let scratch = reg_name(Reg::R13, ty)?;
        let mask = (1u64 << amount) - 1;
        writeln!(w, "\tasr {}, {}, #{}", scratch, dst_reg, width - 1)?;
        writeln!(w, "\tand {}, {}, #{}", scratch, scratch, mask)?;
        writeln!(w, "\tadd {}, {}, {}", dst_reg, dst_reg, scratch)?;
        writeln!(w, "\tasr {}, {}, #{}", dst_reg, dst_reg, amount)?;
        if negate {
            writeln!(w, "\tneg {}, {}", dst_reg, dst_reg)?;
        }
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if binary_xor_negative_one(ty, op, src) {
        writeln!(w, "\tmvn {}, {}", dst_reg, dst_reg)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if let Some(amount) = binary_unsigned_div_power_of_two_amount(ty, op, src) {
        writeln!(w, "\tlsr {}, {}, #{}", dst_reg, dst_reg, amount)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if let Some(mask) = binary_logical_immediate(ty, op, src) {
        writeln!(w, "\t{} {}, {}, #{}", mnemonic, dst_reg, dst_reg, mask)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    if matches!(op, AsmBinaryOp::Adc | AsmBinaryOp::Sbb) && matches!(src, AsmOperand::Imm(0)) {
        let zero = zero_register_for_type(ty)?;
        writeln!(w, "\t{} {}, {}, {}", mnemonic, dst_reg, dst_reg, zero)?;
        return store_operand(w, target, ty, dst_reg, dst);
    }
    let src_reg = load_operand(w, target, ty, src, Reg::R11)?;
    writeln!(w, "\t{} {}, {}, {}", mnemonic, dst_reg, dst_reg, src_reg)?;
    store_operand(w, target, ty, dst_reg, dst)
}

fn binary_add_sub_immediate_offset(op: &AsmBinaryOp, src: &AsmOperand) -> Option<i32> {
    let AsmOperand::Imm(value) = src else {
        return None;
    };
    let offset = match op {
        AsmBinaryOp::Add | AsmBinaryOp::AddSetFlags => *value,
        AsmBinaryOp::Sub | AsmBinaryOp::SubSetFlags => value.checked_neg()?,
        _ => return None,
    };
    let offset = i32::try_from(offset).ok()?;
    (offset != i32::MIN).then_some(offset)
}

fn emit_add_set_flags_immediate(
    w: &mut dyn Write,
    dst_reg: &str,
    offset: i32,
) -> std::io::Result<()> {
    let (mnemonic, magnitude) = if offset >= 0 {
        ("adds", offset as u32)
    } else {
        ("subs", offset.unsigned_abs())
    };
    if magnitude <= 4095 {
        return writeln!(w, "\t{} {}, {}, #{}", mnemonic, dst_reg, dst_reg, magnitude);
    }
    if magnitude % 4096 == 0 && magnitude / 4096 <= 4095 {
        return writeln!(
            w,
            "\t{} {}, {}, #{}, lsl #12",
            mnemonic,
            dst_reg,
            dst_reg,
            magnitude / 4096
        );
    }
    let (ty, scratch) = if dst_reg.starts_with('w') {
        (AsmType::Longword, "w11")
    } else {
        (AsmType::Quadword, "x11")
    };
    emit_load_immediate(w, ty, scratch, i64::from(magnitude))?;
    writeln!(w, "\t{} {}, {}, {}", mnemonic, dst_reg, dst_reg, scratch)
}

fn binary_shift_immediate_amount(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> Option<i64> {
    if !matches!(op, AsmBinaryOp::Sal | AsmBinaryOp::Sar | AsmBinaryOp::Shr) {
        return None;
    }
    let AsmOperand::Imm(amount) = src else {
        return None;
    };
    let width = match ty {
        AsmType::Quadword => 64,
        AsmType::Byte | AsmType::Word | AsmType::Longword => 32,
        _ => return None,
    };
    (0..width).contains(amount).then_some(*amount)
}

fn binary_mul_trivial_value(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> Option<u64> {
    if !matches!(op, AsmBinaryOp::Mul) {
        return None;
    }
    let AsmOperand::Imm(value) = src else {
        return None;
    };
    integer_immediate_value(ty, *value)
        .map(|(value, _)| value)
        .filter(|value| matches!(value, 0 | 1))
}

fn binary_mul_power_of_two_amount(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> Option<u32> {
    if !matches!(op, AsmBinaryOp::Mul) {
        return None;
    }
    let AsmOperand::Imm(value) = src else {
        return None;
    };
    let (value, width) = integer_immediate_value(ty, *value)?;
    let amount = value.trailing_zeros();
    (value.is_power_of_two() && amount < width).then_some(amount)
}

fn binary_mul_negative_one(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> bool {
    if !matches!(op, AsmBinaryOp::Mul) {
        return false;
    }
    let AsmOperand::Imm(value) = src else {
        return false;
    };
    let Some((value, width)) = integer_immediate_value(ty, *value) else {
        return false;
    };
    value
        == if width == 64 {
            u64::MAX
        } else {
            u32::MAX as u64
        }
}

fn binary_mul_negative_power_of_two_amount(
    ty: AsmType,
    op: &AsmBinaryOp,
    src: &AsmOperand,
) -> Option<u32> {
    if !matches!(op, AsmBinaryOp::Mul) {
        return None;
    }
    let AsmOperand::Imm(value) = src else {
        return None;
    };
    if *value >= -1 {
        return None;
    }
    let (_, width) = integer_immediate_value(ty, *value)?;
    let magnitude = value.unsigned_abs();
    let amount = magnitude.trailing_zeros();
    (magnitude.is_power_of_two() && amount < width).then_some(amount)
}

fn binary_unsigned_div_power_of_two_amount(
    ty: AsmType,
    op: &AsmBinaryOp,
    src: &AsmOperand,
) -> Option<u32> {
    if !matches!(op, AsmBinaryOp::UDiv) {
        return None;
    }
    let AsmOperand::Imm(value) = src else {
        return None;
    };
    let (value, width) = integer_immediate_value(ty, *value)?;
    let amount = value.trailing_zeros();
    (value.is_power_of_two() && amount < width).then_some(amount)
}

fn binary_divide_by_one(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> bool {
    if !matches!(op, AsmBinaryOp::SDiv | AsmBinaryOp::UDiv) {
        return false;
    }
    let AsmOperand::Imm(value) = src else {
        return false;
    };
    matches!(integer_immediate_value(ty, *value), Some((1, _)))
}

fn binary_signed_divide_by_negative_one(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> bool {
    if !matches!(op, AsmBinaryOp::SDiv) {
        return false;
    }
    let AsmOperand::Imm(value) = src else {
        return false;
    };
    let Some((value, width)) = integer_immediate_value(ty, *value) else {
        return false;
    };
    value
        == if width == 64 {
            u64::MAX
        } else {
            u32::MAX as u64
        }
}

fn binary_signed_div_power_of_two_amount(
    ty: AsmType,
    op: &AsmBinaryOp,
    src: &AsmOperand,
) -> Option<(u32, u32, bool)> {
    if !matches!(op, AsmBinaryOp::SDiv) {
        return None;
    }
    let AsmOperand::Imm(value) = src else {
        return None;
    };
    let (value, width) = integer_immediate_value(ty, *value)?;
    let value = if width == 64 {
        value as i64
    } else {
        value as u32 as i32 as i64
    };
    let (magnitude, negate) = if value > 1 {
        (value as u64, false)
    } else if value < -1 {
        (value.unsigned_abs(), true)
    } else {
        return None;
    };
    let amount = magnitude.trailing_zeros();
    (magnitude.is_power_of_two() && amount < width - 1).then_some((amount, width, negate))
}

fn binary_logical_noop(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> bool {
    let AsmOperand::Imm(value) = src else {
        return false;
    };
    let Some((value, width)) = integer_immediate_value(ty, *value) else {
        return false;
    };
    let all_ones = if width == 64 {
        u64::MAX
    } else {
        u32::MAX as u64
    };
    matches!((op, value), (AsmBinaryOp::And, v) if v == all_ones)
        || matches!((op, value), (AsmBinaryOp::Or | AsmBinaryOp::Xor, 0))
}

fn binary_logical_zero_result(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> bool {
    matches!(op, AsmBinaryOp::And)
        && matches!(src, AsmOperand::Imm(value) if integer_immediate_value(ty, *value).is_some_and(|(value, _)| value == 0))
}

fn binary_logical_all_ones_result(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> bool {
    if !matches!(op, AsmBinaryOp::Or) {
        return false;
    }
    let AsmOperand::Imm(value) = src else {
        return false;
    };
    let Some((value, width)) = integer_immediate_value(ty, *value) else {
        return false;
    };
    value
        == if width == 64 {
            u64::MAX
        } else {
            u32::MAX as u64
        }
}

fn binary_xor_negative_one(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> bool {
    if !matches!(op, AsmBinaryOp::Xor) {
        return false;
    }
    let AsmOperand::Imm(value) = src else {
        return false;
    };
    let Some((value, width)) = integer_immediate_value(ty, *value) else {
        return false;
    };
    value
        == if width == 64 {
            u64::MAX
        } else {
            u32::MAX as u64
        }
}

fn integer_immediate_value(ty: AsmType, value: i64) -> Option<(u64, u32)> {
    Some(match ty {
        AsmType::Byte | AsmType::Word | AsmType::Longword => (value as u32 as u64, 32),
        AsmType::Quadword => (value as u64, 64),
        _ => return None,
    })
}

fn binary_logical_immediate(ty: AsmType, op: &AsmBinaryOp, src: &AsmOperand) -> Option<u64> {
    if !matches!(op, AsmBinaryOp::And | AsmBinaryOp::Or | AsmBinaryOp::Xor) {
        return None;
    }
    let AsmOperand::Imm(value) = src else {
        return None;
    };
    let (mask, width) = match ty {
        AsmType::Quadword => (*value as u64, 64),
        AsmType::Byte | AsmType::Word | AsmType::Longword => (*value as u32 as u64, 32),
        _ => return None,
    };
    is_aarch64_logical_immediate(mask, width).then_some(mask)
}

/// Whether `value` is representable by AArch64's logical-immediate encoding.
///
/// Such masks are a repeated bitfield whose element is a rotation of one
/// contiguous run of one bits.  Keeping this test here lets all AND/OR/XOR
/// lowering share the ISA's full immediate space instead of special-casing a
/// few common masks.
fn is_aarch64_logical_immediate(value: u64, width: u32) -> bool {
    debug_assert!(matches!(width, 32 | 64));
    let full_mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    if value == 0 || value == full_mask {
        return false;
    }

    for element_width in [2, 4, 8, 16, 32, 64] {
        if element_width > width {
            break;
        }
        let element_mask = if element_width == 64 {
            u64::MAX
        } else {
            (1u64 << element_width) - 1
        };
        let element = value & element_mask;
        if repeat_logical_immediate_element(element, element_width, width) != value {
            continue;
        }
        for one_count in 1..element_width {
            let run = (1u64 << one_count) - 1;
            for rotation in 0..element_width {
                if rotate_logical_immediate_element(run, rotation, element_width) == element {
                    return true;
                }
            }
        }
    }
    false
}

fn repeat_logical_immediate_element(element: u64, element_width: u32, width: u32) -> u64 {
    let mut repeated = 0;
    let mut offset = 0;
    while offset < width {
        repeated |= element << offset;
        offset += element_width;
    }
    repeated
}

fn rotate_logical_immediate_element(value: u64, rotation: u32, width: u32) -> u64 {
    if rotation == 0 {
        return value;
    }
    let mask = if width == 64 {
        u64::MAX
    } else {
        (1u64 << width) - 1
    };
    ((value >> rotation) | (value << (width - rotation))) & mask
}

fn emit_cmp(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    if matches!(ty, AsmType::Float | AsmType::Double) {
        if matches!(src, AsmOperand::Imm(0)) {
            let dst_reg = load_operand(w, target, ty, dst, Reg::R10)?;
            return writeln!(w, "\tfcmp {}, #0.0", dst_reg);
        }
        if matches!(dst, AsmOperand::Imm(0)) {
            let src_reg = load_operand(w, target, ty, src, Reg::R10)?;
            return writeln!(w, "\tfcmp {}, #0.0", src_reg);
        }
        let dst_reg = load_operand(w, target, ty, dst, Reg::R10)?;
        let src_reg = load_operand(w, target, ty, src, Reg::R11)?;
        return writeln!(w, "\tfcmp {}, {}", dst_reg, src_reg);
    }

    let dst_reg = load_operand(w, target, ty, dst, Reg::R10)?;
    if let Some((immediate, shift)) = cmp_immediate(src) {
        if shift == 0 {
            return writeln!(w, "\tcmp {}, #{}", dst_reg, immediate);
        }
        return writeln!(w, "\tcmp {}, #{}, lsl #{}", dst_reg, immediate, shift);
    }
    if let AsmOperand::Imm(value) = src {
        if let Some((immediate, shift)) = value.checked_neg().and_then(cmp_immediate_value) {
            if shift == 0 {
                return writeln!(w, "\tcmn {}, #{}", dst_reg, immediate);
            }
            return writeln!(w, "\tcmn {}, #{}, lsl #{}", dst_reg, immediate, shift);
        }
    }
    let src_reg = load_operand(w, target, ty, src, Reg::R11)?;
    writeln!(w, "\tcmp {}, {}", dst_reg, src_reg)
}

fn cmp_immediate(src: &AsmOperand) -> Option<(i64, u8)> {
    match src {
        AsmOperand::Imm(value) => cmp_immediate_value(*value),
        _ => None,
    }
}

fn cmp_immediate_value(value: i64) -> Option<(i64, u8)> {
    if (0..=4095).contains(&value) {
        return Some((value, 0));
    }
    (value > 0 && value % 4096 == 0 && value / 4096 <= 4095).then_some((value / 4096, 12))
}

fn emit_lea(
    w: &mut dyn Write,
    target: &Target,
    src: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let direct_dst = match dst {
        AsmOperand::Reg(reg) => Some(reg_name(*reg, AsmType::Quadword)?),
        _ => None,
    };
    let addr_reg = direct_dst.unwrap_or("x9");
    match src {
        AsmOperand::Stack(offset) => {
            if *offset == 0 {
                writeln!(w, "\tmov {}, sp", addr_reg)?;
            } else {
                emit_stack_address_into(w, addr_reg, stack_offset_i32(*offset)?)?;
            }
            store_operand(w, target, AsmType::Quadword, addr_reg, dst)
        }
        AsmOperand::Data(name) => {
            emit_load_data_address(w, target, name, addr_reg)?;
            store_operand(w, target, AsmType::Quadword, addr_reg, dst)
        }
        AsmOperand::TlsData(name, offset) => {
            emit_load_tls_address(w, target, name, *offset, addr_reg)?;
            store_operand(w, target, AsmType::Quadword, addr_reg, dst)
        }
        other => invalid_input(format!(
            "AArch64 backend cannot take address of operand yet: {:?}",
            other
        )),
    }
}

fn emit_operand_address_into(
    w: &mut dyn Write,
    target: &Target,
    src: &AsmOperand,
    dst_reg: &'static str,
) -> std::io::Result<()> {
    match src {
        AsmOperand::Stack(offset) => {
            emit_stack_address_into(w, dst_reg, stack_offset_i32(*offset)?)
        }
        AsmOperand::Data(name) => emit_load_data_address(w, target, name, dst_reg),
        AsmOperand::TlsData(name, offset) => {
            emit_load_tls_address(w, target, name, *offset, dst_reg)
        }
        other => invalid_input(format!(
            "AArch64 backend cannot take address of operand yet: {:?}",
            other
        )),
    }
}

/// Copy a known-size aggregate between the source address in `x11` and the
/// destination address in `x12`.
///
/// Small argument copies are common and do not need a loop counter or branch.
/// For larger aggregates, copy full sixteen-byte units in a compact pair-load
/// loop and finish with exact-width tail moves, avoiding both byte-at-a-time
/// traffic and reads past the end of the object.
fn emit_byte_copy_loop(w: &mut dyn Write, size: usize) -> std::io::Result<()> {
    const INLINE_COPY_LIMIT: usize = 32;

    if size <= INLINE_COPY_LIMIT {
        return emit_copy_tail(w, size);
    }

    let pairs = size / 16;
    debug_assert!(pairs > 0);
    emit_load_immediate(w, AsmType::Quadword, "x13", pairs as i64)?;
    writeln!(w, "1:")?;
    emit_copy_pair(w)?;
    writeln!(w, "\tsubs x13, x13, #1")?;
    writeln!(w, "\tb.ne 1b")?;
    emit_copy_tail(w, size % 16)
}

fn emit_copy_tail(w: &mut dyn Write, mut size: usize) -> std::io::Result<()> {
    while size >= 16 {
        emit_copy_pair(w)?;
        size -= 16;
    }
    for (reg, load, store, width) in [
        ("x10", "ldr", "str", 8),
        ("w10", "ldr", "str", 4),
        ("w10", "ldrh", "strh", 2),
        ("w10", "ldrb", "strb", 1),
    ] {
        while size >= width {
            emit_copy_chunk(w, reg, load, store, width)?;
            size -= width;
        }
    }
    debug_assert_eq!(size, 0);
    Ok(())
}

fn emit_copy_pair(w: &mut dyn Write) -> std::io::Result<()> {
    writeln!(w, "\tldp x10, x14, [x11], #16")?;
    writeln!(w, "\tstp x10, x14, [x12], #16")
}

fn emit_copy_chunk(
    w: &mut dyn Write,
    reg: &str,
    load: &str,
    store: &str,
    width: usize,
) -> std::io::Result<()> {
    writeln!(w, "\t{} {}, [x11], #{}", load, reg, width)?;
    writeln!(w, "\t{} {}, [x12], #{}", store, reg, width)
}

fn emit_copy_to_stack_arg(
    w: &mut dyn Write,
    target: &Target,
    src_ptr: &AsmOperand,
    dst_offset: i32,
    size: usize,
) -> std::io::Result<()> {
    let src = load_operand(w, target, AsmType::Quadword, src_ptr, Reg::R11)?;
    if src != "x11" {
        writeln!(w, "\tmov x11, {}", src)?;
    }
    emit_stack_address_into(w, "x12", dst_offset)?;
    emit_byte_copy_loop(w, size)
}

fn emit_copy_from_stack_arg(
    w: &mut dyn Write,
    target: &Target,
    src_offset: i32,
    dst: &AsmOperand,
    size: usize,
) -> std::io::Result<()> {
    emit_stack_address_into(w, "x11", src_offset)?;
    emit_operand_address_into(w, target, dst, "x12")?;
    emit_byte_copy_loop(w, size)
}

fn emit_load_indirect(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    base: Reg,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let base_reg = reg_name(base, AsmType::Quadword)?;
    if ty == AsmType::Octword {
        if matches!(dst, AsmOperand::Reg(Reg::AX)) {
            writeln!(w, "\tldr x0, [{}]", base_reg)?;
            return writeln!(w, "\tldr x1, [{}, #8]", base_reg);
        }
        let dst_low = offset_operand(dst, 0)?;
        let dst_high = offset_operand(dst, 8)?;
        writeln!(w, "\tldr x9, [{}]", base_reg)?;
        writeln!(w, "\tldr x11, [{}, #8]", base_reg)?;
        store_operand(w, target, AsmType::Quadword, "x9", &dst_low)?;
        return store_operand(w, target, AsmType::Quadword, "x11", &dst_high);
    }
    match dst {
        AsmOperand::Reg(reg) => {
            return writeln!(
                w,
                "\t{} {}, [{}]",
                load_mnemonic(ty),
                reg_name(*reg, ty)?,
                base_reg
            );
        }
        AsmOperand::Xmm(reg) => {
            return writeln!(
                w,
                "\t{} {}, [{}]",
                load_mnemonic(ty),
                fp_name_typed(*reg, ty)?,
                base_reg
            );
        }
        _ => {}
    }
    let scratch = if matches!(ty, AsmType::Float | AsmType::Double) {
        fp_scratch_name_typed(Reg::R10, ty)?
    } else {
        reg_name(Reg::R10, ty)?
    };
    writeln!(w, "\t{} {}, [{}]", load_mnemonic(ty), scratch, base_reg)?;
    store_operand(w, target, ty, scratch, dst)
}

fn emit_store_indirect(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src: &AsmOperand,
    base: Reg,
) -> std::io::Result<()> {
    let base_reg = reg_name(base, AsmType::Quadword)?;
    if ty == AsmType::Octword {
        let src_low = offset_operand(src, 0)?;
        let src_high = offset_operand(src, 8)?;
        let low = load_operand(w, target, AsmType::Quadword, &src_low, Reg::R10)?;
        let high = load_operand(w, target, AsmType::Quadword, &src_high, Reg::R13)?;
        writeln!(w, "\tstr {}, [{}]", low, base_reg)?;
        return writeln!(w, "\tstr {}, [{}, #8]", high, base_reg);
    }
    let scratch = load_operand(w, target, ty, src, Reg::R10)?;
    writeln!(w, "\t{} {}, [{}]", store_mnemonic(ty), scratch, base_reg)
}

fn emit_add_ptr(
    w: &mut dyn Write,
    target: &Target,
    ptr: &AsmOperand,
    index: &AsmOperand,
    scale: i64,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let ptr_reg = load_operand(w, target, AsmType::Quadword, ptr, Reg::R10)?;
    if let AsmOperand::Imm(index) = index {
        let offset = index.wrapping_mul(scale);
        if offset == 0 {
            return store_operand(w, target, AsmType::Quadword, ptr_reg, dst);
        }
        if let Ok(offset) = i32::try_from(offset) {
            if offset != i32::MIN {
                emit_add_immediate(w, ptr_reg, offset)?;
                return store_operand(w, target, AsmType::Quadword, ptr_reg, dst);
            }
        }
        emit_load_immediate(w, AsmType::Quadword, "x11", offset)?;
        writeln!(w, "\tadd {}, {}, x11", ptr_reg, ptr_reg)?;
        return store_operand(w, target, AsmType::Quadword, ptr_reg, dst);
    }

    let index_reg = load_operand(w, target, AsmType::Quadword, index, Reg::R11)?;
    if scale == 1 {
        writeln!(w, "\tadd {}, {}, {}", ptr_reg, ptr_reg, index_reg)?;
    } else if let Some(shift) = scaled_add_shift(scale) {
        writeln!(
            w,
            "\tadd {}, {}, {}, lsl #{}",
            ptr_reg, ptr_reg, index_reg, shift
        )?;
    } else {
        emit_load_immediate(w, AsmType::Quadword, "x11", scale)?;
        writeln!(w, "\tmadd {}, {}, x11, {}", ptr_reg, index_reg, ptr_reg)?;
    }
    store_operand(w, target, AsmType::Quadword, ptr_reg, dst)
}

fn emit_extract(
    w: &mut dyn Write,
    target: &Target,
    high: &AsmOperand,
    low: &AsmOperand,
    lsb: u8,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    if !(1..64).contains(&lsb) {
        return invalid_input(format!("invalid AArch64 extr shift amount: {lsb}"));
    }
    let high_reg = load_operand(w, target, AsmType::Quadword, high, Reg::R10)?;
    let low_reg = load_operand(w, target, AsmType::Quadword, low, Reg::R11)?;
    let dst_reg = reg_name(Reg::R13, AsmType::Quadword)?;
    writeln!(w, "\textr {}, {}, {}, #{}", dst_reg, high_reg, low_reg, lsb)?;
    store_operand(w, target, AsmType::Quadword, dst_reg, dst)
}

fn emit_umulh(
    w: &mut dyn Write,
    target: &Target,
    left: &AsmOperand,
    right: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    let left_reg = load_operand(w, target, AsmType::Quadword, left, Reg::R10)?;
    let right_reg = load_operand(w, target, AsmType::Quadword, right, Reg::R11)?;
    let dst_reg = match dst {
        AsmOperand::Reg(reg) => reg_name(*reg, AsmType::Quadword)?,
        _ => reg_name(Reg::R13, AsmType::Quadword)?,
    };
    writeln!(w, "\tumulh {}, {}, {}", dst_reg, left_reg, right_reg)?;
    store_operand(w, target, AsmType::Quadword, dst_reg, dst)
}

fn scaled_add_shift(scale: i64) -> Option<u32> {
    let scale = u64::try_from(scale).ok()?;
    (scale > 1 && scale.is_power_of_two()).then_some(scale.trailing_zeros())
}

fn load_operand_rebased(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    operand: &AsmOperand,
    scratch: Reg,
    stack_rebase: i32,
) -> std::io::Result<&'static str> {
    match operand {
        AsmOperand::Stack(offset) => {
            let reg = if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
                fp_scratch_name_typed(scratch, ty)?
            } else {
                reg_name(scratch, ty)?
            };
            emit_load_stack(
                w,
                ty,
                reg,
                stack_offset_i32(*offset + i64::from(stack_rebase))?,
            )?;
            Ok(reg)
        }
        _ => load_operand(w, target, ty, operand, scratch),
    }
}

fn emit_store_outgoing_arg(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src: &AsmOperand,
    outgoing_offset: i32,
    stack_rebase: i32,
) -> std::io::Result<()> {
    let scratch = load_operand_rebased(w, target, ty, src, Reg::R10, stack_rebase)?;
    emit_store_stack(w, ty, scratch, outgoing_offset)
}

fn emit_load_adjusted(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    src: &AsmOperand,
    dst: Reg,
    stack_rebase: i32,
) -> std::io::Result<()> {
    let loaded = load_operand_rebased(w, target, ty, src, dst, stack_rebase)?;
    let dst_reg = reg_name(dst, ty)?;
    if loaded == dst_reg {
        Ok(())
    } else {
        writeln!(w, "\tmov {}, {}", dst_reg, loaded)
    }
}

fn emit_int_to_double(
    w: &mut dyn Write,
    target: &Target,
    src_ty: AsmType,
    src: &AsmOperand,
    dst: &AsmOperand,
    unsigned: bool,
    dst_ty: AsmType,
) -> std::io::Result<()> {
    if matches!(src, AsmOperand::Imm(0)) {
        let zero_reg = zero_register_for_type(dst_ty)?;
        let dst_reg = match dst {
            AsmOperand::Xmm(reg) => fp_name_typed(*reg, dst_ty)?,
            _ => fp_scratch_name_typed(Reg::R10, dst_ty)?,
        };
        writeln!(w, "\tfmov {}, {}", dst_reg, zero_reg)?;
        return store_operand(w, target, dst_ty, dst_reg, dst);
    }
    let src_reg = load_operand(w, target, src_ty, src, Reg::R10)?;
    let dst_reg = match dst {
        AsmOperand::Xmm(reg) => fp_name_typed(*reg, dst_ty)?,
        _ => fp_scratch_name_typed(Reg::R10, dst_ty)?,
    };
    let mnemonic = if unsigned { "ucvtf" } else { "scvtf" };
    writeln!(w, "\t{} {}, {}", mnemonic, dst_reg, src_reg)?;
    store_operand(w, target, dst_ty, dst_reg, dst)
}

fn emit_double_to_int(
    w: &mut dyn Write,
    target: &Target,
    dst_ty: AsmType,
    src: &AsmOperand,
    dst: &AsmOperand,
    unsigned: bool,
    src_ty: AsmType,
) -> std::io::Result<()> {
    if matches!(src, AsmOperand::Imm(0)) {
        let zero_reg = zero_register_for_type(dst_ty)?;
        let dst_reg = match dst {
            AsmOperand::Reg(reg) => reg_name(*reg, dst_ty)?,
            _ => reg_name(Reg::R10, dst_ty)?,
        };
        writeln!(w, "\tmov {}, {}", dst_reg, zero_reg)?;
        return store_operand(w, target, dst_ty, dst_reg, dst);
    }
    let src_reg = load_operand(w, target, src_ty, src, Reg::R10)?;
    let dst_reg = match dst {
        AsmOperand::Reg(reg) => reg_name(*reg, dst_ty)?,
        _ => reg_name(Reg::R10, dst_ty)?,
    };
    let mnemonic = if unsigned { "fcvtzu" } else { "fcvtzs" };
    writeln!(w, "\t{} {}, {}", mnemonic, dst_reg, src_reg)?;
    store_operand(w, target, dst_ty, dst_reg, dst)
}

fn emit_float_convert(
    w: &mut dyn Write,
    target: &Target,
    src_ty: AsmType,
    dst_ty: AsmType,
    src: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    if matches!(src, AsmOperand::Imm(0)) {
        let zero_reg = zero_register_for_type(dst_ty)?;
        let dst_reg = match dst {
            AsmOperand::Xmm(reg) => fp_name_typed(*reg, dst_ty)?,
            _ => fp_scratch_name_typed(Reg::R11, dst_ty)?,
        };
        writeln!(w, "\tfmov {}, {}", dst_reg, zero_reg)?;
        return store_operand(w, target, dst_ty, dst_reg, dst);
    }
    let src_reg = load_operand(w, target, src_ty, src, Reg::R10)?;
    let dst_reg = match dst {
        AsmOperand::Xmm(reg) => fp_name_typed(*reg, dst_ty)?,
        _ => fp_scratch_name_typed(Reg::R11, dst_ty)?,
    };
    writeln!(w, "\tfcvt {}, {}", dst_reg, src_reg)?;
    store_operand(w, target, dst_ty, dst_reg, dst)
}

fn emit_remainder(
    w: &mut dyn Write,
    target: &Target,
    ty: AsmType,
    is_unsigned: bool,
    left: &AsmOperand,
    right: &AsmOperand,
    dst: &AsmOperand,
) -> std::io::Result<()> {
    if remainder_is_trivially_zero(ty, is_unsigned, right) {
        return store_operand(w, target, ty, zero_register_for_type(ty)?, dst);
    }
    let left_reg = load_operand(w, target, ty, left, Reg::R10)?;
    if is_unsigned {
        if let Some(mask) = unsigned_remainder_power_of_two_mask(ty, right) {
            if mask == 0 {
                let zero = zero_register_for_type(ty)?;
                writeln!(w, "\tmov {}, {}", left_reg, zero)?;
            } else {
                writeln!(w, "\tand {}, {}, #{}", left_reg, left_reg, mask)?;
            }
            return store_operand(w, target, ty, left_reg, dst);
        }
    }
    if !is_unsigned {
        if let Some((amount, width, _)) =
            binary_signed_div_power_of_two_amount(ty, &AsmBinaryOp::SDiv, right)
        {
            let quotient_reg = reg_name(Reg::R13, ty)?;
            let mask = (1u64 << amount) - 1;
            writeln!(w, "\tasr {}, {}, #{}", quotient_reg, left_reg, width - 1)?;
            writeln!(w, "\tand {}, {}, #{}", quotient_reg, quotient_reg, mask)?;
            writeln!(w, "\tadd {}, {}, {}", quotient_reg, left_reg, quotient_reg)?;
            writeln!(w, "\tasr {}, {}, #{}", quotient_reg, quotient_reg, amount)?;
            writeln!(w, "\tlsl {}, {}, #{}", quotient_reg, quotient_reg, amount)?;
            writeln!(w, "\tsub {}, {}, {}", left_reg, left_reg, quotient_reg)?;
            return store_operand(w, target, ty, left_reg, dst);
        }
    }
    let right_reg = load_operand(w, target, ty, right, Reg::R11)?;
    let quotient_reg = reg_name(Reg::R13, ty)?;
    let div_mnemonic = if is_unsigned { "udiv" } else { "sdiv" };
    writeln!(
        w,
        "\t{} {}, {}, {}",
        div_mnemonic, quotient_reg, left_reg, right_reg
    )?;
    writeln!(
        w,
        "\tmsub {}, {}, {}, {}",
        left_reg, quotient_reg, right_reg, left_reg
    )?;
    store_operand(w, target, ty, left_reg, dst)
}

fn remainder_is_trivially_zero(ty: AsmType, is_unsigned: bool, right: &AsmOperand) -> bool {
    let AsmOperand::Imm(value) = right else {
        return false;
    };
    let Some((value, width)) = integer_immediate_value(ty, *value) else {
        return false;
    };
    value == 1
        || (!is_unsigned
            && value
                == if width == 64 {
                    u64::MAX
                } else {
                    u32::MAX as u64
                })
}

fn unsigned_remainder_power_of_two_mask(ty: AsmType, right: &AsmOperand) -> Option<u64> {
    let AsmOperand::Imm(value) = right else {
        return None;
    };
    let (value, width) = integer_immediate_value(ty, *value)?;
    let amount = value.trailing_zeros();
    if !value.is_power_of_two() || amount >= width {
        return None;
    }
    Some(value - 1)
}

fn emit_instruction(w: &mut dyn Write, instr: &AsmInstr, target: &Target) -> std::io::Result<()> {
    match instr {
        AsmInstr::Mov(ty, src, dst) => emit_mov(w, target, *ty, src, dst),
        AsmInstr::Movsx(src_ty, dst_ty, src, dst) => {
            emit_movsx(w, target, *src_ty, *dst_ty, src, dst)
        }
        AsmInstr::MovZeroExtend(src_ty, dst_ty, src, dst) => {
            emit_mov_zero_extend(w, target, *src_ty, *dst_ty, src, dst)
        }
        AsmInstr::Cvtsi2sd(src_ty, src, dst) => {
            emit_int_to_double(w, target, *src_ty, src, dst, false, AsmType::Double)
        }
        AsmInstr::Cvtsi2ss(src_ty, src, dst) => {
            emit_int_to_double(w, target, *src_ty, src, dst, false, AsmType::Float)
        }
        AsmInstr::Cvttsd2si(dst_ty, src, dst) => {
            emit_double_to_int(w, target, *dst_ty, src, dst, false, AsmType::Double)
        }
        AsmInstr::Cvttss2si(dst_ty, src, dst) => {
            emit_double_to_int(w, target, *dst_ty, src, dst, false, AsmType::Float)
        }
        AsmInstr::AArch64UIntToDouble(src_ty, src, dst) => {
            emit_int_to_double(w, target, *src_ty, src, dst, true, AsmType::Double)
        }
        AsmInstr::AArch64UIntToFloat(src_ty, src, dst) => {
            emit_int_to_double(w, target, *src_ty, src, dst, true, AsmType::Float)
        }
        AsmInstr::AArch64DoubleToUInt(dst_ty, src, dst) => {
            emit_double_to_int(w, target, *dst_ty, src, dst, true, AsmType::Double)
        }
        AsmInstr::AArch64FloatToUInt(dst_ty, src, dst) => {
            emit_double_to_int(w, target, *dst_ty, src, dst, true, AsmType::Float)
        }
        AsmInstr::AArch64FloatToDouble(src, dst) => {
            emit_float_convert(w, target, AsmType::Float, AsmType::Double, src, dst)
        }
        AsmInstr::AArch64DoubleToFloat(src, dst) => {
            emit_float_convert(w, target, AsmType::Double, AsmType::Float, src, dst)
        }
        AsmInstr::Cvtss2sd(..) | AsmInstr::Cvtsd2ss(..) => {
            invalid_input("x86 floating conversion instruction reached AArch64 emitter")
        }
        AsmInstr::Unary(ty, op, dst) => emit_unary(w, target, *ty, op, dst),
        AsmInstr::Binary(ty, op, src, dst) => emit_binary(w, target, *ty, op, src, dst),
        AsmInstr::AtomicRmw(ty, op, return_old, dst) => {
            emit_atomic_rmw(w, target, *ty, op, *return_old, dst)
        }
        AsmInstr::AtomicExchange(ty, dst) => emit_atomic_exchange(w, target, *ty, dst),
        AsmInstr::AtomicCompareExchange(ty, dst) => {
            emit_atomic_compare_exchange(w, target, *ty, dst)
        }
        AsmInstr::AtomicCompareSwap(ty, return_old, dst) => {
            emit_atomic_compare_swap(w, target, *ty, *return_old, dst)
        }
        AsmInstr::Cmp(ty, src, dst) => emit_cmp(w, target, *ty, src, dst),
        AsmInstr::Lea(src, dst) => emit_lea(w, target, src, dst),
        AsmInstr::LoadIndirect(ty, base, dst) => emit_load_indirect(w, target, *ty, *base, dst),
        AsmInstr::StoreIndirect(ty, src, base) => emit_store_indirect(w, target, *ty, src, *base),
        AsmInstr::CopyToStackArg {
            src_ptr,
            dst_offset,
            size,
        } => emit_copy_to_stack_arg(w, target, src_ptr, *dst_offset, *size),
        AsmInstr::CopyFromStackArg {
            src_offset,
            dst,
            size,
        } => emit_copy_from_stack_arg(w, target, *src_offset, dst, *size),
        AsmInstr::Jmp(label) => writeln!(w, "\tb .L{}", label),
        AsmInstr::NonlocalJmp(label) => writeln!(w, "\tb .L{}", label),
        AsmInstr::JmpIndirect(target_op) => {
            let reg = load_operand(w, target, AsmType::Quadword, target_op, Reg::R10)?;
            writeln!(w, "\tbr {}", reg)
        }
        AsmInstr::JmpCC(cc, label) => {
            writeln!(w, "\tb.{} 1f", inverse_condition_name(cc))?;
            writeln!(w, "\tb .L{}", label)?;
            writeln!(w, "1:")
        }
        AsmInstr::SetCC(cc, dst) => {
            if let AsmOperand::Reg(reg) = dst {
                writeln!(
                    w,
                    "\tcset {}, {}",
                    reg_name(*reg, AsmType::Longword)?,
                    condition_name(cc)
                )
            } else {
                writeln!(w, "\tcset w9, {}", condition_name(cc))?;
                store_operand(w, target, AsmType::Longword, "w9", dst)
            }
        }
        AsmInstr::Label(label) => writeln!(w, ".L{}:", label),
        AsmInstr::LoadLabelAddress(label, dst) => {
            let dst_reg = match dst {
                AsmOperand::Reg(reg) => reg_name(*reg, AsmType::Quadword)?,
                _ => "x9",
            };
            writeln!(w, "\tadrp {}, .L{}@PAGE", dst_reg, label)?;
            writeln!(w, "\tadd {}, {}, .L{}@PAGEOFF", dst_reg, dst_reg, label)?;
            store_operand(w, target, AsmType::Quadword, dst_reg, dst)
        }
        AsmInstr::BuiltinSetjmp {
            buf,
            dst,
            label,
            end_label,
        } => {
            let buf_reg = load_operand(w, target, AsmType::Quadword, buf, Reg::R11)?;
            writeln!(w, "\tadrp x9, .L{}@PAGE", label)?;
            writeln!(w, "\tadd x9, x9, .L{}@PAGEOFF", label)?;
            writeln!(w, "\tstr x9, [{}]", buf_reg)?;
            writeln!(w, "\tadd x10, {}, #8", buf_reg)?;
            writeln!(w, "\tmov x9, sp")?;
            writeln!(w, "\tstr x9, [x10]")?;
            writeln!(w, "\tadd x10, {}, #16", buf_reg)?;
            writeln!(w, "\tstr x30, [x10]")?;
            store_operand(w, target, AsmType::Longword, "wzr", dst)?;
            writeln!(w, "\tb .L{}", end_label)?;
            writeln!(w, ".L{}:", label)?;
            writeln!(w, "\tmov w9, #1")?;
            store_operand(w, target, AsmType::Longword, "w9", dst)?;
            writeln!(w, ".L{}:", end_label)
        }
        AsmInstr::BuiltinLongjmp { buf, value: _ } => {
            let buf_reg = load_operand(w, target, AsmType::Quadword, buf, Reg::R10)?;
            writeln!(w, "\tldr x12, [{}]", buf_reg)?;
            writeln!(w, "\tldr x11, [{}, #8]", buf_reg)?;
            writeln!(w, "\tldr x30, [{}, #16]", buf_reg)?;
            writeln!(w, "\tmov sp, x11")?;
            writeln!(w, "\tbr x12")
        }
        AsmInstr::Call(name, _, _, false, _) => writeln!(w, "\tbl {}", target.show_symbol(name)),
        AsmInstr::Call(_, _, _, true, _) => writeln!(w, "\tblr x9"),
        AsmInstr::AArch64AddPtr(ptr, index, scale, dst) => {
            emit_add_ptr(w, target, ptr, index, *scale, dst)
        }
        AsmInstr::AArch64Extr(high, low, lsb, dst) => emit_extract(w, target, high, low, *lsb, dst),
        AsmInstr::AArch64Umulh(left, right, dst) => emit_umulh(w, target, left, right, dst),
        AsmInstr::AArch64LoadAdjusted(ty, src, dst, stack_rebase) => {
            emit_load_adjusted(w, target, *ty, src, *dst, *stack_rebase)
        }
        AsmInstr::AArch64StoreOutgoingArg(ty, src, outgoing_offset, stack_rebase) => {
            emit_store_outgoing_arg(w, target, *ty, src, *outgoing_offset, *stack_rebase)
        }
        AsmInstr::AArch64Rem(ty, is_unsigned, left, right, dst) => {
            emit_remainder(w, target, *ty, *is_unsigned, left, right, dst)
        }
        AsmInstr::X86SetVarargsXmmCount(_) => {
            invalid_input("x86-64 varargs instruction reached AArch64 emitter")
        }
        AsmInstr::AArch64SaveLink(offset) => emit_store_stack(w, AsmType::Quadword, "x30", *offset),
        AsmInstr::AArch64RestoreLink(offset) => {
            emit_load_stack(w, AsmType::Quadword, "x30", *offset)
        }
        AsmInstr::AArch64AllocateLargeStack(bytes) => {
            emit_large_stack_pointer_adjust(w, "sub", *bytes)
        }
        AsmInstr::AArch64DeallocateLargeStack(bytes) => {
            emit_large_stack_pointer_adjust(w, "add", *bytes)
        }
        AsmInstr::AArch64StoreLargeLocalBase {
            base_offset,
            dst_offset,
        } => emit_store_large_local_base(w, *base_offset, *dst_offset),
        AsmInstr::AtomicFence => writeln!(w, "\tdmb ish"),
        AsmInstr::AllocateStack(bytes) if *bytes > 0 => emit_stack_pointer_adjust(w, "sub", *bytes),
        AsmInstr::DeallocateStack(bytes) if *bytes > 0 => {
            emit_stack_pointer_adjust(w, "add", *bytes)
        }
        AsmInstr::AllocateStack(_) | AsmInstr::DeallocateStack(_) => Ok(()),
        AsmInstr::Unreachable => writeln!(w, "\tbrk #0"),
        AsmInstr::Ret => writeln!(w, "\tret"),
        other => invalid_input(format!(
            "AArch64 backend cannot emit instruction yet: {:?}",
            other
        )),
    }
}

fn emit_function(
    w: &mut dyn Write,
    function: &AsmFunction,
    target: &Target,
) -> std::io::Result<()> {
    let name = target.show_symbol(&function.name);
    writeln!(w, "\t.text")?;
    if function.global {
        writeln!(w, "\t.globl {}", name)?;
    }
    writeln!(w, "{}:", name)?;
    let instructions = &function.instructions;
    let mut index = 0;
    while index < instructions.len() {
        if let (
            AsmInstr::Cmp(ty @ (AsmType::Longword | AsmType::Quadword), AsmOperand::Imm(0), value),
            Some(AsmInstr::JmpCC(cc @ (CondCode::E | CondCode::NE), label)),
        ) = (&instructions[index], instructions.get(index + 1))
        {
            if matches!(
                value,
                AsmOperand::Reg(_)
                    | AsmOperand::Stack(_)
                    | AsmOperand::Data(_)
                    | AsmOperand::TlsData(_, _)
            ) {
                let mnemonic = if matches!(cc, CondCode::E) {
                    "cbnz"
                } else {
                    "cbz"
                };
                let value_reg = load_operand(w, target, *ty, value, Reg::R10)?;
                writeln!(w, "\t{} {}, 1f", mnemonic, value_reg)?;
                writeln!(w, "\tb .L{}", label)?;
                writeln!(w, "1:")?;
                index += 2;
                continue;
            }
        }
        emit_instruction(w, &instructions[index], target)?;
        index += 1;
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
            b if (0x20..0x7f).contains(&b) => out.push(b as char),
            b => out.push_str(&format!("\\{:03o}", b)),
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

fn static_init_size(init: &StaticInit) -> usize {
    match init {
        StaticInit::CharInit(_) | StaticInit::UCharInit(_) => 1,
        StaticInit::ShortInit(_) | StaticInit::UShortInit(_) => 2,
        StaticInit::IntInit(_) | StaticInit::UIntInit(_) => 4,
        StaticInit::LongInit(_)
        | StaticInit::ULongInit(_)
        | StaticInit::DoubleInit(_)
        | StaticInit::PointerInit(_)
        | StaticInit::PointerInitOffset(_, _) => 8,
        StaticInit::LabelDiffInit(_, _, bytes) => *bytes,
        StaticInit::Int128Init(_) | StaticInit::UInt128Init(_) | StaticInit::LongDoubleInit(_) => {
            16
        }
        StaticInit::FloatInit(_) => 4,
        StaticInit::ZeroInit(n) => *n,
        StaticInit::StringInit(s, null_terminated) => {
            c_string_byte_len(s) + usize::from(*null_terminated)
        }
    }
}

fn alignment_log2(alignment: usize) -> usize {
    alignment.next_power_of_two().trailing_zeros() as usize
}

fn data_alignment(alignment: usize) -> usize {
    alignment.max(8)
}

fn emit_macho_tls_static_var(
    w: &mut dyn Write,
    sv: &AsmStaticVar,
    target: &Target,
    all_zero: bool,
) -> std::io::Result<()> {
    let label = target.show_symbol(&sv.name);
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
            emit_static_init(w, init, target)?;
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

fn binary128_from_f64(value: f64) -> u128 {
    let bits = value.to_bits();
    let sign = ((bits >> 63) as u128) << 127;
    let exp = ((bits >> 52) & 0x7ff) as i32;
    let frac = (bits & ((1u64 << 52) - 1)) as u128;
    if exp == 0 {
        if frac == 0 {
            return sign;
        }
        let top_bit = 127 - frac.leading_zeros() as i32;
        let normalized = frac << (63 - top_bit);
        let unbiased = top_bit - 1074;
        let exponent = (unbiased + 16383) as u128;
        return sign | (exponent << 112) | ((normalized & ((1u128 << 63) - 1)) << 49);
    }
    if exp == 0x7ff {
        return sign | (0x7fffu128 << 112) | (frac << 60);
    }
    let exponent = (exp - 1023 + 16383) as u128;
    sign | (exponent << 112) | (frac << 60)
}

fn emit_static_init(w: &mut dyn Write, init: &StaticInit, target: &Target) -> std::io::Result<()> {
    match init {
        StaticInit::CharInit(v) => writeln!(w, "\t.byte {}", *v as u8),
        StaticInit::UCharInit(v) => writeln!(w, "\t.byte {}", v),
        StaticInit::ShortInit(v) => writeln!(w, "\t.short {}", v),
        StaticInit::UShortInit(v) => writeln!(w, "\t.short {}", v),
        StaticInit::IntInit(v) => writeln!(w, "\t.long {}", v),
        StaticInit::UIntInit(v) => writeln!(w, "\t.long {}", v),
        StaticInit::LongInit(v) => writeln!(w, "\t.quad {}", v),
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
            let bits = binary128_from_f64(*v);
            writeln!(w, "\t.quad {}", bits as u64)?;
            writeln!(w, "\t.quad {}", (bits >> 64) as u64)
        }
        StaticInit::ZeroInit(n) => writeln!(w, "\t.zero {}", n),
        StaticInit::StringInit(s, null_terminated) => emit_string_init(w, s, *null_terminated),
        StaticInit::PointerInit(label) => {
            writeln!(w, "\t.quad {}", static_label_name(target, label))
        }
        StaticInit::PointerInitOffset(label, offset) => {
            let sign = if *offset >= 0 { "+" } else { "" };
            writeln!(
                w,
                "\t.quad {}{}{}",
                static_label_name(target, label),
                sign,
                offset
            )
        }
        StaticInit::LabelDiffInit(left, right, 1) => writeln!(
            w,
            "\t.byte {}-{}",
            static_label_name(target, left),
            static_label_name(target, right)
        ),
        StaticInit::LabelDiffInit(left, right, 2) => writeln!(
            w,
            "\t.short {}-{}",
            static_label_name(target, left),
            static_label_name(target, right)
        ),
        StaticInit::LabelDiffInit(left, right, 4) => writeln!(
            w,
            "\t.long {}-{}",
            static_label_name(target, left),
            static_label_name(target, right)
        ),
        StaticInit::LabelDiffInit(left, right, 8) => writeln!(
            w,
            "\t.quad {}-{}",
            static_label_name(target, left),
            static_label_name(target, right)
        ),
        StaticInit::LabelDiffInit(_, _, bytes) => invalid_input(format!(
            "unsupported label difference initializer size: {bytes}"
        )),
    }
}

fn emit_static_var(w: &mut dyn Write, sv: &AsmStaticVar, target: &Target) -> std::io::Result<()> {
    let label = target.show_symbol(&sv.name);
    let alignment = data_alignment(sv.alignment);
    let all_zero = !sv.init_values.is_empty()
        && sv
            .init_values
            .iter()
            .all(|init| matches!(init, StaticInit::ZeroInit(_)));

    if sv.thread_local && target.os == TargetOs::Linux {
        if all_zero {
            writeln!(w, "\t.section .tbss,\"awT\",@nobits")?;
        } else {
            writeln!(w, "\t.section .tdata,\"awT\",@progbits")?;
        }
    } else if sv.thread_local && target.os == TargetOs::MacOs {
        return emit_macho_tls_static_var(w, sv, target, all_zero);
    } else if all_zero && target.os == TargetOs::MacOs {
        if sv.global {
            writeln!(w, "\t.globl {}", label)?;
        }
        let size: usize = sv.init_values.iter().map(static_init_size).sum();
        return writeln!(
            w,
            "\t.zerofill __DATA,__bss,{},{},{}",
            label,
            size,
            alignment_log2(alignment)
        );
    }

    if all_zero {
        writeln!(w, "\t.bss")?;
    } else {
        writeln!(w, "\t.data")?;
    }
    if sv.global {
        writeln!(w, "\t.globl {}", label)?;
    }
    writeln!(w, "\t.balign {}", alignment)?;
    writeln!(w, "{}:", label)?;
    for init in &sv.init_values {
        emit_static_init(w, init, target)?;
    }
    Ok(())
}

fn emit_static_constant(
    w: &mut dyn Write,
    sc: &AsmStaticConstant,
    target: &Target,
) -> std::io::Result<()> {
    match target.os {
        TargetOs::Linux => writeln!(w, "\t.section .rodata")?,
        TargetOs::MacOs if matches!(&sc.init, StaticInit::StringInit(s, _) if !c_string_bytes(s).contains(&0)) => {
            writeln!(w, "\t.section __TEXT,__cstring,cstring_literals")?
        }
        TargetOs::MacOs => writeln!(w, "\t.section __TEXT,__const")?,
    }
    if sc.alignment > 1 {
        writeln!(w, "\t.balign {}", sc.alignment)?;
    }
    writeln!(w, "{}:", target.show_symbol(&sc.name))?;
    emit_static_init(w, &sc.init, target)
}

fn emit_alias(
    w: &mut dyn Write,
    name: &str,
    alias_target: &str,
    target: &Target,
) -> std::io::Result<()> {
    if alias_target.is_empty() {
        return Ok(());
    }
    let name = target.show_symbol(name);
    let alias_target = target.show_symbol(alias_target);
    writeln!(w, "\t.globl {}", name)?;
    writeln!(w, "\t.set {}, {}", name, alias_target)
}

fn emit_stack_note(w: &mut dyn Write, target: &Target) -> std::io::Result<()> {
    match target.os {
        TargetOs::Linux => writeln!(w, "\t.section .note.GNU-stack,\"\",@progbits"),
        TargetOs::MacOs => Ok(()),
    }
}

pub fn emit(assembly_file: &str, program: &AsmProgram, target: &Target) -> std::io::Result<()> {
    let mut file = std::fs::File::create(assembly_file)?;
    for item in &program.top_level {
        match item {
            AsmTopLevel::Function(function) => emit_function(&mut file, function, target)?,
            AsmTopLevel::StaticVar(sv) => emit_static_var(&mut file, sv, target)?,
            AsmTopLevel::StaticConstant(sc) => emit_static_constant(&mut file, sc, target)?,
            AsmTopLevel::Alias {
                name,
                target: alias_target,
            } => emit_alias(&mut file, name, alias_target, target)?,
        }
    }
    emit_stack_note(&mut file, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_copy_uses_exact_width_moves_for_small_sizes() -> Result<(), String> {
        let mut out = Vec::new();
        emit_byte_copy_loop(&mut out, 15).map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tldr x10, [x11], #8\n\
             \tstr x10, [x12], #8\n\
             \tldr w10, [x11], #4\n\
             \tstr w10, [x12], #4\n\
             \tldrh w10, [x11], #2\n\
             \tstrh w10, [x12], #2\n\
             \tldrb w10, [x11], #1\n\
             \tstrb w10, [x12], #1\n"
        );
        Ok(())
    }

    #[test]
    fn aggregate_copy_uses_pair_moves_before_tail() -> Result<(), String> {
        let mut out = Vec::new();
        emit_byte_copy_loop(&mut out, 24).map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tldp x10, x14, [x11], #16\n\
             \tstp x10, x14, [x12], #16\n\
             \tldr x10, [x11], #8\n\
             \tstr x10, [x12], #8\n"
        );
        Ok(())
    }

    #[test]
    fn aggregate_copy_uses_qword_loop_with_exact_tail() -> Result<(), String> {
        let mut out = Vec::new();
        emit_byte_copy_loop(&mut out, 37).map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tmovz x13, #2\n\
             1:\n\
             \tldp x10, x14, [x11], #16\n\
             \tstp x10, x14, [x12], #16\n\
             \tsubs x13, x13, #1\n\
             \tb.ne 1b\n\
             \tldr w10, [x11], #4\n\
             \tstr w10, [x12], #4\n\
             \tldrb w10, [x11], #1\n\
             \tstrb w10, [x12], #1\n"
        );
        Ok(())
    }

    #[test]
    fn scalar_shift_immediate_uses_immediate_encoding() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Imm(12),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tlsr x0, x0, #12\n");
        Ok(())
    }

    #[test]
    fn cross_limb_extract_preserves_high_low_operand_order() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64Extr(
                AsmOperand::Reg(Reg::DI),
                AsmOperand::Reg(Reg::AX),
                13,
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\textr x11, x1, x0, #13\n\tmov x0, x11\n");
        Ok(())
    }

    #[test]
    fn unsigned_multiply_high_uses_direct_register_destination() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64Umulh(
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R11),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tumulh x10, x9, x12\n");
        Ok(())
    }

    #[test]
    fn integer_multiply_immediates_use_shift_or_trivial_forms() -> Result<(), String> {
        let mut out = Vec::new();
        for source in [
            AsmOperand::Imm(8),
            AsmOperand::Imm(0),
            AsmOperand::Imm(1),
            AsmOperand::Imm(-1),
        ] {
            emit_instruction(
                &mut out,
                &AsmInstr::Binary(
                    AsmType::Quadword,
                    AsmBinaryOp::Mul,
                    source,
                    AsmOperand::Reg(Reg::AX),
                ),
                &Target::aarch64_linux(),
            )
            .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tlsl x0, x0, #3\n\tmov x0, xzr\n\tneg x0, x0\n");
        Ok(())
    }

    #[test]
    fn trivial_logical_immediates_avoid_materializing_constants() -> Result<(), String> {
        let mut out = Vec::new();
        for (op, source) in [
            (AsmBinaryOp::And, AsmOperand::Imm(0)),
            (AsmBinaryOp::And, AsmOperand::Imm(-1)),
            (AsmBinaryOp::Or, AsmOperand::Imm(0)),
            (AsmBinaryOp::Or, AsmOperand::Imm(-1)),
            (AsmBinaryOp::Xor, AsmOperand::Imm(0)),
            (AsmBinaryOp::Xor, AsmOperand::Imm(-1)),
        ] {
            emit_instruction(
                &mut out,
                &AsmInstr::Binary(AsmType::Quadword, op, source, AsmOperand::Reg(Reg::AX)),
                &Target::aarch64_linux(),
            )
            .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tmov x0, xzr\n\tmovn x0, #0\n\tmvn x0, x0\n");
        Ok(())
    }

    #[test]
    fn carry_chain_immediates_use_native_encodings() -> Result<(), String> {
        let mut out = Vec::new();
        for instr in [
            AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::AddSetFlags,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Adc,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::DI),
            ),
        ] {
            emit_instruction(&mut out, &instr, &Target::aarch64_linux())
                .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tadds x0, x0, #1\n\tadcs x1, x1, xzr\n");
        Ok(())
    }

    #[test]
    fn non_power_of_two_pointer_scales_use_madd() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64AddPtr(
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Reg(Reg::DI),
                3,
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tmovz x11, #3\n\tmadd x0, x1, x11, x0\n");
        Ok(())
    }

    #[test]
    fn unsigned_divide_by_power_of_two_uses_shift() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Binary(
                AsmType::Longword,
                AsmBinaryOp::UDiv,
                AsmOperand::Imm(8),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tlsr w0, w0, #3\n");
        Ok(())
    }

    #[test]
    fn signed_divide_by_power_of_two_uses_bias_and_shift() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::SDiv,
                AsmOperand::Imm(8),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tasr x11, x0, #63\n\tand x11, x11, #7\n\tadd x0, x0, x11\n\tasr x0, x0, #3\n"
        );
        Ok(())
    }

    #[test]
    fn signed_divide_by_negative_power_of_two_uses_bias_shift_and_negate() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::SDiv,
                AsmOperand::Imm(-8),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tasr x11, x0, #63\n\tand x11, x11, #7\n\tadd x0, x0, x11\n\tasr x0, x0, #3\n\tneg x0, x0\n"
        );
        Ok(())
    }

    #[test]
    fn integer_divide_by_one_is_elided() -> Result<(), String> {
        let mut out = Vec::new();
        for op in [AsmBinaryOp::SDiv, AsmBinaryOp::UDiv] {
            emit_instruction(
                &mut out,
                &AsmInstr::Binary(
                    AsmType::Longword,
                    op,
                    AsmOperand::Imm(1),
                    AsmOperand::Reg(Reg::AX),
                ),
                &Target::aarch64_linux(),
            )
            .map_err(|err| err.to_string())?;
        }

        assert!(out.is_empty());
        Ok(())
    }

    #[test]
    fn signed_divide_by_negative_one_uses_negate() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::SDiv,
                AsmOperand::Imm(-1),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tneg x0, x0\n");
        Ok(())
    }

    #[test]
    fn unsigned_remainder_by_power_of_two_uses_mask() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64Rem(
                AsmType::Longword,
                true,
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Imm(8),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tand w0, w0, #7\n");
        Ok(())
    }

    #[test]
    fn signed_remainder_by_power_of_two_uses_bias_and_shift() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64Rem(
                AsmType::Quadword,
                false,
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Imm(8),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tasr x11, x0, #63\n\tand x11, x11, #7\n\tadd x11, x0, x11\n\tasr x11, x11, #3\n\tlsl x11, x11, #3\n\tsub x0, x0, x11\n"
        );
        Ok(())
    }

    #[test]
    fn signed_remainder_by_negative_power_of_two_uses_bias_and_shift() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64Rem(
                AsmType::Quadword,
                false,
                AsmOperand::Reg(Reg::AX),
                AsmOperand::Imm(-8),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tasr x11, x0, #63\n\tand x11, x11, #7\n\tadd x11, x0, x11\n\tasr x11, x11, #3\n\tlsl x11, x11, #3\n\tsub x0, x0, x11\n"
        );
        Ok(())
    }

    #[test]
    fn trivial_integer_remainders_use_zero() -> Result<(), String> {
        let mut out = Vec::new();
        for (is_unsigned, divisor) in [(true, 1), (false, 1), (false, -1)] {
            emit_instruction(
                &mut out,
                &AsmInstr::AArch64Rem(
                    AsmType::Quadword,
                    is_unsigned,
                    AsmOperand::Reg(Reg::AX),
                    AsmOperand::Imm(divisor),
                    AsmOperand::Reg(Reg::AX),
                ),
                &Target::aarch64_linux(),
            )
            .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tmov x0, xzr\n\tmov x0, xzr\n\tmov x0, xzr\n");
        Ok(())
    }

    #[test]
    fn low_bit_mask_uses_logical_immediate_encoding() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Binary(
                AsmType::Longword,
                AsmBinaryOp::And,
                AsmOperand::Imm(31),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tand w0, w0, #31\n");
        Ok(())
    }

    #[test]
    fn single_bit_logical_ops_use_immediate_encoding() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Xor,
                AsmOperand::Imm(1 << 40),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\teor x0, x0, #1099511627776\n");
        Ok(())
    }

    #[test]
    fn repeated_rotated_logical_masks_use_immediate_encoding() -> Result<(), String> {
        let mut out = Vec::new();
        for instr in [
            AsmInstr::Binary(
                AsmType::Longword,
                AsmBinaryOp::And,
                AsmOperand::Imm(0x00ff_00ff),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Or,
                AsmOperand::Imm(0x00ff_00ff_00ff_00ff),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Binary(
                AsmType::Longword,
                AsmBinaryOp::Xor,
                AsmOperand::Imm(-16_711_936),
                AsmOperand::Reg(Reg::AX),
            ),
        ] {
            emit_instruction(&mut out, &instr, &Target::aarch64_linux())
                .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tand w0, w0, #16711935\n\
             \torr x0, x0, #71777214294589695\n\
             \teor w0, w0, #4278255360\n"
        );
        Ok(())
    }

    #[test]
    fn logical_immediate_recognizer_accepts_only_encodable_masks() {
        assert!(is_aarch64_logical_immediate(0x00ff_00ff, 32));
        assert!(is_aarch64_logical_immediate(0xff00_ff00_ff00_ff00, 64));
        assert!(is_aarch64_logical_immediate(u64::MAX - 1, 64));
        assert!(!is_aarch64_logical_immediate(0, 64));
        assert!(!is_aarch64_logical_immediate(u64::MAX, 64));
        assert!(!is_aarch64_logical_immediate(0x0123_4567, 32));
    }

    #[test]
    fn large_stack_offsets_use_materialized_address() -> Result<(), String> {
        let mut out = Vec::new();
        let function = AsmFunction {
            name: "main".to_string(),
            global: true,
            instructions: vec![
                AsmInstr::AllocateStack(100_048),
                AsmInstr::AArch64SaveLink(100_040),
                AsmInstr::AArch64RestoreLink(100_040),
                AsmInstr::DeallocateStack(100_048),
                AsmInstr::Ret,
            ],
        };

        emit_function(&mut out, &function, &Target::aarch64_linux())
            .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert!(asm.contains("sub sp, sp, #24, lsl #12"));
        assert!(asm.contains("sub sp, sp, #1744"));
        assert!(asm.contains("add x16, sp, x16"));
        assert!(asm.contains("str x30, [x16]"));
        assert!(asm.contains("ldr x30, [x16]"));
        assert!(asm.contains("add sp, sp, #24, lsl #12"));
        assert!(asm.contains("add sp, sp, #1744"));
        Ok(())
    }

    #[test]
    fn zero_compare_branch_uses_cbz_or_cbnz() -> Result<(), String> {
        let function = AsmFunction {
            name: "f".to_string(),
            global: false,
            instructions: vec![
                AsmInstr::Cmp(
                    AsmType::Longword,
                    AsmOperand::Imm(0),
                    AsmOperand::Reg(Reg::AX),
                ),
                AsmInstr::JmpCC(CondCode::E, "zero".to_string()),
                AsmInstr::Cmp(
                    AsmType::Quadword,
                    AsmOperand::Imm(0),
                    AsmOperand::Reg(Reg::DI),
                ),
                AsmInstr::JmpCC(CondCode::NE, "nonzero".to_string()),
                AsmInstr::Cmp(AsmType::Longword, AsmOperand::Imm(0), AsmOperand::Stack(16)),
                AsmInstr::JmpCC(CondCode::E, "stack_zero".to_string()),
                AsmInstr::Ret,
            ],
        };
        let mut out = Vec::new();
        emit_function(&mut out, &function, &Target::aarch64_linux())
            .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert!(asm.contains("\tcbnz w0, 1f\n\tb .Lzero\n"), "{asm}");
        assert!(asm.contains("\tcbz x1, 1f\n\tb .Lnonzero\n"), "{asm}");
        assert!(
            asm.contains("\tldr w9, [sp, #16]\n\tcbnz w9, 1f\n\tb .Lstack_zero\n"),
            "{asm}"
        );
        assert!(!asm.contains("cmp w0, #0"), "{asm}");
        assert!(!asm.contains("cmp x1, #0"), "{asm}");
        assert!(!asm.contains("cmp w9, #0"), "{asm}");
        Ok(())
    }

    #[test]
    fn zero_extend_to_128_bits_stores_high_half_from_xzr() -> Result<(), String> {
        let mut out = Vec::new();
        emit_mov_zero_extend(
            &mut out,
            &Target::aarch64_linux(),
            AsmType::Longword,
            AsmType::Octword,
            &AsmOperand::Reg(Reg::AX),
            &AsmOperand::Stack(16),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert!(asm.contains("str xzr, [sp, #24]"), "{asm}");
        assert!(!asm.contains("mov x11, #0"), "{asm}");
        Ok(())
    }

    #[test]
    fn zero_immediate_uses_zero_register() -> Result<(), String> {
        let mut out = Vec::new();
        emit_load_immediate(&mut out, AsmType::Quadword, "x9", 0).map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tmov x9, xzr\n");
        Ok(())
    }

    #[test]
    fn integer_immediates_choose_shortest_move_wide_sequence() -> Result<(), String> {
        let mut out = Vec::new();
        emit_load_immediate(&mut out, AsmType::Quadword, "x9", -1)
            .map_err(|err| err.to_string())?;
        emit_load_immediate(&mut out, AsmType::Quadword, "x10", 1 << 32)
            .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert!(asm.contains("movn x9, #0"), "{asm}");
        assert!(asm.contains("movz x10, #1, lsl #32"), "{asm}");
        assert_eq!(asm.lines().count(), 2, "{asm}");
        Ok(())
    }

    #[test]
    fn float_zero_immediate_uses_fmov_from_zero_register() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Mov(
                AsmType::Double,
                AsmOperand::Imm(0),
                AsmOperand::Xmm(XmmReg::XMM0),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tfmov d0, xzr\n");
        Ok(())
    }

    #[test]
    fn common_float_immediates_use_direct_fmov_encoding() -> Result<(), String> {
        let mut out = Vec::new();
        for instr in [
            AsmInstr::Mov(
                AsmType::Float,
                AsmOperand::Imm(0x3f80_0000),
                AsmOperand::Xmm(XmmReg::XMM0),
            ),
            AsmInstr::Mov(
                AsmType::Float,
                AsmOperand::Imm(0xbf00_0000),
                AsmOperand::Xmm(XmmReg::XMM1),
            ),
            AsmInstr::Mov(
                AsmType::Double,
                AsmOperand::Imm(0xbff0_0000_0000_0000_u64 as i64),
                AsmOperand::Xmm(XmmReg::XMM2),
            ),
            AsmInstr::Mov(
                AsmType::Double,
                AsmOperand::Imm(0x401c_0000_0000_0000),
                AsmOperand::Xmm(XmmReg::XMM3),
            ),
            AsmInstr::Mov(
                AsmType::Float,
                AsmOperand::Imm(0x3fc0_0000),
                AsmOperand::Xmm(XmmReg::XMM4),
            ),
            AsmInstr::Mov(
                AsmType::Double,
                AsmOperand::Imm(0x3fc0_0000_0000_0000),
                AsmOperand::Xmm(XmmReg::XMM5),
            ),
            AsmInstr::Mov(
                AsmType::Double,
                AsmOperand::Imm(0xc03f_0000_0000_0000_u64 as i64),
                AsmOperand::Xmm(XmmReg::XMM6),
            ),
        ] {
            emit_instruction(&mut out, &instr, &Target::aarch64_linux())
                .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tfmov s0, #1.0\n\
             \tfmov s1, #-0.5\n\
             \tfmov d2, #-1.0\n\
             \tfmov d3, #7.0\n\
             \tfmov s4, #1.5\n\
             \tfmov d5, #0.125\n\
             \tfmov d6, #-31.0\n"
        );
        Ok(())
    }

    #[test]
    fn fmov_immediate_recognizes_full_encoding_space() {
        for exponent in -3..=4 {
            for fraction in 0..16_u32 {
                let float_bits = ((exponent + 127) as u32) << 23 | fraction << 19;
                let double_bits = ((exponent + 1023) as u64) << 52 | (fraction as u64) << 48;
                for signed in [float_bits as i64, (float_bits | (1 << 31)) as i64] {
                    assert!(aarch64_fmov_immediate(AsmType::Float, signed).is_some());
                }
                for signed in [double_bits as i64, (double_bits | (1 << 63)) as i64] {
                    assert!(aarch64_fmov_immediate(AsmType::Double, signed).is_some());
                }
            }
        }
        assert!(aarch64_fmov_immediate(AsmType::Float, 0x3dcc_cccd).is_none());
        assert!(aarch64_fmov_immediate(AsmType::Double, 0x3fb9_9999_9999_999a).is_none());
    }

    #[test]
    fn float_compare_against_zero_uses_fcmp_zero() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Cmp(
                AsmType::Double,
                AsmOperand::Imm(0),
                AsmOperand::Xmm(XmmReg::XMM0),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tfcmp d0, #0.0\n");
        Ok(())
    }

    #[test]
    fn integer_compare_immediates_avoid_register_materialization() -> Result<(), String> {
        let mut out = Vec::new();
        for instr in [
            AsmInstr::Cmp(
                AsmType::Longword,
                AsmOperand::Imm(42),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Cmp(
                AsmType::Quadword,
                AsmOperand::Imm(0xabc000),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Cmp(
                AsmType::Longword,
                AsmOperand::Imm(-42),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Cmp(
                AsmType::Quadword,
                AsmOperand::Imm(-0xabc000),
                AsmOperand::Reg(Reg::AX),
            ),
        ] {
            emit_instruction(&mut out, &instr, &Target::aarch64_linux())
                .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tcmp w0, #42\n\
             \tcmp x0, #2748, lsl #12\n\
             \tcmn w0, #42\n\
             \tcmn x0, #2748, lsl #12\n"
        );
        assert_eq!(cmp_immediate(&AsmOperand::Imm(-1)), None);
        Ok(())
    }

    #[test]
    fn integer_add_sub_uses_shifted_immediate_encoding() -> Result<(), String> {
        let mut out = Vec::new();
        for instr in [
            AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Add,
                AsmOperand::Imm(0xabc000),
                AsmOperand::Reg(Reg::AX),
            ),
            AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sub,
                AsmOperand::Imm(0xabc000),
                AsmOperand::Reg(Reg::AX),
            ),
        ] {
            emit_instruction(&mut out, &instr, &Target::aarch64_linux())
                .map_err(|err| err.to_string())?;
        }
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(
            asm,
            "\tadd x0, x0, #2748, lsl #12\n\
             \tsub x0, x0, #2748, lsl #12\n"
        );
        Ok(())
    }

    #[test]
    fn float_to_double_zero_uses_direct_fmov() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64FloatToDouble(AsmOperand::Imm(0), AsmOperand::Xmm(XmmReg::XMM0)),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tfmov d0, xzr\n");
        Ok(())
    }

    #[test]
    fn double_to_float_zero_uses_direct_fmov() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AArch64DoubleToFloat(AsmOperand::Imm(0), AsmOperand::Xmm(XmmReg::XMM0)),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tfmov s0, wzr\n");
        Ok(())
    }

    #[test]
    fn int_to_float_zero_uses_direct_fmov() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Cvtsi2ss(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Xmm(XmmReg::XMM0),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tfmov s0, wzr\n");
        Ok(())
    }

    #[test]
    fn int_to_double_zero_uses_direct_fmov() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Cvtsi2sd(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Xmm(XmmReg::XMM0),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tfmov d0, xzr\n");
        Ok(())
    }

    #[test]
    fn float_to_int_zero_uses_direct_zeroing() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Cvttsd2si(
                AsmType::Longword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tmov w0, wzr\n");
        Ok(())
    }

    #[test]
    fn float_to_uint64_zero_uses_direct_zeroing() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::Cvttss2si(
                AsmType::Quadword,
                AsmOperand::Imm(0),
                AsmOperand::Reg(Reg::AX),
            ),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert_eq!(asm, "\tmov x0, xzr\n");
        Ok(())
    }

    #[test]
    fn atomic_compare_exchange_zeros_status_with_wzr() -> Result<(), String> {
        let mut out = Vec::new();
        emit_instruction(
            &mut out,
            &AsmInstr::AtomicCompareExchange(AsmType::Longword, AsmOperand::Reg(Reg::AX)),
            &Target::aarch64_linux(),
        )
        .map_err(|err| err.to_string())?;
        let asm = String::from_utf8(out).map_err(|err| err.to_string())?;

        assert!(asm.contains("mov w15, wzr"), "{asm}");
        assert!(!asm.contains("mov w15, #0"), "{asm}");
        Ok(())
    }
}
