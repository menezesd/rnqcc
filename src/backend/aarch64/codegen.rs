use crate::types::*;
use std::collections::{HashMap, HashSet};

const ARG_REGS: [Reg; 8] = [
    Reg::AX,
    Reg::DI,
    Reg::SI,
    Reg::DX,
    Reg::CX,
    Reg::R8,
    Reg::R9,
    Reg::R12,
];
const FP_ARG_REGS: [XmmReg; 8] = [
    XmmReg::XMM0,
    XmmReg::XMM1,
    XmmReg::XMM2,
    XmmReg::XMM3,
    XmmReg::XMM4,
    XmmReg::XMM5,
    XmmReg::XMM6,
    XmmReg::XMM7,
];
const STACK_ALIGNMENT: i32 = 16;
const STACK_SLOT_SIZE: i32 = 8;
const MIN_STACK_SLOT_SIZE: i32 = 4;
const LINK_REGISTER_SAVE_SIZE: i32 = 16;
const LARGE_LOCAL_ALIGNMENT: usize = 16;

#[derive(Debug, Clone)]
struct LargeLocal {
    name: String,
    size: usize,
    base_slot: i32,
}

#[derive(Debug)]
struct StackLayout {
    slots: HashMap<String, i32>,
    local_stack_size: i32,
    large_locals: Vec<LargeLocal>,
}

fn align_to(value: i32, alignment: i32) -> i32 {
    if value == 0 {
        0
    } else {
        (value + alignment - 1) & !(alignment - 1)
    }
}

fn compute_frame_size(local_stack_size: i32, saves_link_register: bool) -> i32 {
    if saves_link_register {
        local_stack_size + LINK_REGISTER_SAVE_SIZE
    } else {
        local_stack_size
    }
}

fn compute_link_register_offset(frame_size: i32) -> i32 {
    frame_size - STACK_SLOT_SIZE
}

fn align_usize_to(value: usize, alignment: usize) -> Result<usize, String> {
    if alignment == 0 {
        return Ok(value);
    }
    value
        .checked_add(alignment - 1)
        .map(|rounded| rounded & !(alignment - 1))
        .ok_or_else(|| "AArch64 backend large local alignment overflow".to_string())
}

fn align_i64_to(value: i64, alignment: i64) -> Result<i64, String> {
    if alignment <= 0 {
        return Ok(value);
    }
    value
        .checked_add(alignment - 1)
        .map(|rounded| rounded & !(alignment - 1))
        .ok_or_else(|| "AArch64 backend large stack alignment overflow".to_string())
}

fn stack_arg_offset(base: i32, index: usize) -> i32 {
    base + (index as i32 * STACK_SLOT_SIZE)
}

fn outgoing_stack_size(stack_arg_count: usize) -> i32 {
    align_to(stack_arg_count as i32 * STACK_SLOT_SIZE, STACK_ALIGNMENT)
}

fn emit_epilogue(
    instructions: &mut Vec<AsmInstr>,
    frame_size: i32,
    large_stack_size: i64,
    link_register_offset: Option<i32>,
) {
    if let Some(offset) = link_register_offset {
        instructions.push(AsmInstr::AArch64RestoreLink(offset));
    }
    if frame_size > 0 {
        instructions.push(AsmInstr::DeallocateStack(frame_size));
    }
    if large_stack_size > 0 {
        instructions.push(AsmInstr::AArch64DeallocateLargeStack(large_stack_size));
    }
    instructions.push(AsmInstr::Ret);
}

fn i128_parts_signed(value: i128) -> (i64, i64) {
    (value as i64, (value >> 64) as i64)
}

fn i128_parts_unsigned(value: u128) -> (i64, i64) {
    (value as u64 as i64, (value >> 64) as u64 as i64)
}

fn low64_operand(op: &AsmOperand) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Stack(offset) => Ok(AsmOperand::Stack(*offset)),
        AsmOperand::Data(name) => Ok(AsmOperand::Data(name.clone())),
        AsmOperand::Reg(reg) => Ok(AsmOperand::Reg(*reg)),
        AsmOperand::Imm(value) => Ok(AsmOperand::Imm(*value)),
        other => Err(format!(
            "AArch64 backend cannot address low half of {:?}",
            other
        )),
    }
}

fn high64_operand(op: &AsmOperand) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Stack(offset) => Ok(AsmOperand::Stack(*offset + 8)),
        AsmOperand::Data(name) => Ok(data_operand_with_offset(name, 8)),
        AsmOperand::Reg(Reg::AX) => Ok(AsmOperand::Reg(Reg::DI)),
        AsmOperand::Reg(reg) => Err(format!(
            "AArch64 backend cannot address high half of 128-bit register {:?}",
            reg
        )),
        AsmOperand::Imm(_) => {
            Err("AArch64 backend cannot address high half of immediate".to_string())
        }
        other => Err(format!(
            "AArch64 backend cannot address high half of {:?}",
            other
        )),
    }
}

fn byte_offset_operand(op: &AsmOperand, offset: i32) -> Result<AsmOperand, String> {
    match op {
        AsmOperand::Stack(base) => Ok(AsmOperand::Stack(base + offset)),
        AsmOperand::Data(name) => Ok(data_operand_with_offset(name, offset)),
        other => Err(format!(
            "AArch64 backend cannot address byte offset {} of {:?}",
            offset, other
        )),
    }
}

fn i128_part_operands(
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(AsmOperand, AsmOperand), String> {
    match val {
        TackyVal::Constant(value) => Ok((
            AsmOperand::Imm(*value),
            AsmOperand::Imm(if *value < 0 { -1 } else { 0 }),
        )),
        TackyVal::Int128Constant(value) => {
            let (low, high) = i128_parts_signed(*value);
            Ok((AsmOperand::Imm(low), AsmOperand::Imm(high)))
        }
        TackyVal::UInt128Constant(value) => {
            let (low, high) = i128_parts_unsigned(*value);
            Ok((AsmOperand::Imm(low), AsmOperand::Imm(high)))
        }
        _ => {
            let op = val_operand(val, stack_slots, global_vars)?;
            Ok((low64_operand(&op)?, high64_operand(&op)?))
        }
    }
}

fn emit_i128_copy(
    instructions: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let dst_op = val_operand(dst, stack_slots, global_vars)?;
    let dst_low = low64_operand(&dst_op)?;
    let dst_high = high64_operand(&dst_op)?;
    match src {
        TackyVal::Int128Constant(value) => {
            let (low, high) = i128_parts_signed(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                dst_low,
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                dst_high,
            ));
        }
        TackyVal::UInt128Constant(value) => {
            let (low, high) = i128_parts_unsigned(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                dst_low,
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                dst_high,
            ));
        }
        _ => {
            let (src_low, src_high) = i128_part_operands(src, stack_slots, global_vars)?;
            instructions.push(AsmInstr::Mov(AsmType::Quadword, src_low, dst_low));
            instructions.push(AsmInstr::Mov(AsmType::Quadword, src_high, dst_high));
        }
    }
    Ok(())
}

fn emit_i128_zero_cmp(
    instructions: &mut Vec<AsmInstr>,
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let (low, high) = i128_part_operands(val, stack_slots, global_vars)?;
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        low,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Or,
        high,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::R10),
    ));
    Ok(())
}

struct Aarch64I128Context<'a> {
    function_name: &'a str,
    types: &'a HashMap<String, CType>,
    stack_slots: &'a HashMap<String, i32>,
    global_vars: &'a HashSet<String>,
}

fn emit_i128_signed_cmp(
    instructions: &mut Vec<AsmInstr>,
    left: &TackyVal,
    right: &TackyVal,
    op: &TackyBinaryOp,
    dst: AsmOperand,
    ctx: &Aarch64I128Context<'_>,
) -> Result<(), String> {
    let (left_low, left_high) = i128_part_operands(left, ctx.stack_slots, ctx.global_vars)?;
    let (right_low, right_high) = i128_part_operands(right, ctx.stack_slots, ctx.global_vars)?;
    let id = instructions.len();
    let true_label = format!("i128_cmp_true.{}.{}", ctx.function_name, id);
    let end_label = format!("i128_cmp_end.{}.{}", ctx.function_name, id);
    let (high_true, high_false, low_true) = match op {
        TackyBinaryOp::GreaterThan => (CondCode::G, CondCode::L, CondCode::A),
        TackyBinaryOp::GreaterEqual => (CondCode::G, CondCode::L, CondCode::AE),
        TackyBinaryOp::LessThan => (CondCode::L, CondCode::G, CondCode::B),
        TackyBinaryOp::LessEqual => (CondCode::L, CondCode::G, CondCode::BE),
        _ => return Err(format!("unsupported 128-bit signed comparison: {:?}", op)),
    };

    instructions.push(AsmInstr::Mov(
        AsmType::Longword,
        AsmOperand::Imm(0),
        dst.clone(),
    ));
    instructions.push(AsmInstr::Cmp(AsmType::Quadword, right_high, left_high));
    instructions.push(AsmInstr::JmpCC(high_true, true_label.clone()));
    instructions.push(AsmInstr::JmpCC(high_false, end_label.clone()));
    instructions.push(AsmInstr::Cmp(AsmType::Quadword, right_low, left_low));
    instructions.push(AsmInstr::JmpCC(low_true, true_label.clone()));
    instructions.push(AsmInstr::Jmp(end_label.clone()));
    instructions.push(AsmInstr::Label(true_label));
    instructions.push(AsmInstr::Mov(AsmType::Longword, AsmOperand::Imm(1), dst));
    instructions.push(AsmInstr::Label(end_label));
    Ok(())
}

fn emit_i128_return(
    instructions: &mut Vec<AsmInstr>,
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    match val {
        TackyVal::Constant(value) => {
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(*value),
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(if *value < 0 { -1 } else { 0 }),
                AsmOperand::Reg(Reg::DI),
            ));
        }
        TackyVal::Int128Constant(value) => {
            let (low, high) = i128_parts_signed(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                AsmOperand::Reg(Reg::DI),
            ));
        }
        TackyVal::UInt128Constant(value) => {
            let (low, high) = i128_parts_unsigned(*value);
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(low),
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Imm(high),
                AsmOperand::Reg(Reg::DI),
            ));
        }
        _ => {
            let src = val_operand(val, stack_slots, global_vars)?;
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                low64_operand(&src)?,
                AsmOperand::Reg(Reg::AX),
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                high64_operand(&src)?,
                AsmOperand::Reg(Reg::DI),
            ));
        }
    }
    Ok(())
}

fn emit_i128_variable_shift(
    instructions: &mut Vec<AsmInstr>,
    op: &TackyBinaryOp,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    ctx: &Aarch64I128Context<'_>,
) -> Result<(), String> {
    let (left_low, left_high) = i128_part_operands(left, ctx.stack_slots, ctx.global_vars)?;
    let dst_op = val_operand(dst, ctx.stack_slots, ctx.global_vars)?;
    let dst_low = low64_operand(&dst_op)?;
    let dst_high = high64_operand(&dst_op)?;
    let right_ty = asm_type_for_val(right, ctx.types)?;
    let amount_src = if right_ty == AsmType::Octword {
        low64_operand(&val_operand(right, ctx.stack_slots, ctx.global_vars)?)?
    } else {
        val_operand(right, ctx.stack_slots, ctx.global_vars)?
    };
    let id = instructions.len();
    let loop_label = format!("i128_shift_loop.{}.{}", ctx.function_name, id);
    let end_label = format!("i128_shift_end.{}.{}", ctx.function_name, id);

    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_low,
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        left_high,
        AsmOperand::Reg(Reg::R13),
    ));
    instructions.push(AsmInstr::Mov(
        right_ty,
        amount_src,
        AsmOperand::Reg(Reg::R12),
    ));
    instructions.push(AsmInstr::Label(loop_label.clone()));
    instructions.push(AsmInstr::Cmp(
        AsmType::Quadword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::R12),
    ));
    instructions.push(AsmInstr::JmpCC(CondCode::E, end_label.clone()));

    match op {
        TackyBinaryOp::ShiftLeft => {
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R10),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Imm(63),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Or,
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R13),
            ));
        }
        TackyBinaryOp::ShiftRight => {
            let high_shift = if is_unsigned_val(left, ctx.types) {
                AsmBinaryOp::Shr
            } else {
                AsmBinaryOp::Sar
            };
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(Reg::R13),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Sal,
                AsmOperand::Imm(63),
                AsmOperand::Reg(Reg::R14),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Shr,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R10),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                high_shift,
                AsmOperand::Imm(1),
                AsmOperand::Reg(Reg::R13),
            ));
            instructions.push(AsmInstr::Binary(
                AsmType::Quadword,
                AsmBinaryOp::Or,
                AsmOperand::Reg(Reg::R14),
                AsmOperand::Reg(Reg::R10),
            ));
        }
        _ => return Err("internal error: expected i128 shift op".to_string()),
    }

    instructions.push(AsmInstr::Binary(
        AsmType::Quadword,
        AsmBinaryOp::Sub,
        AsmOperand::Imm(1),
        AsmOperand::Reg(Reg::R12),
    ));
    instructions.push(AsmInstr::Jmp(loop_label));
    instructions.push(AsmInstr::Label(end_label));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        AsmOperand::Reg(Reg::R10),
        dst_low,
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Quadword,
        AsmOperand::Reg(Reg::R13),
        dst_high,
    ));
    Ok(())
}

fn asm_type_for_val(val: &TackyVal, types: &HashMap<String, CType>) -> Result<AsmType, String> {
    match val {
        TackyVal::Constant(c) => {
            if *c > i32::MAX as i64 || *c < i32::MIN as i64 {
                Ok(AsmType::Quadword)
            } else {
                Ok(AsmType::Longword)
            }
        }
        TackyVal::Int128Constant(_) | TackyVal::UInt128Constant(_) => Ok(AsmType::Octword),
        TackyVal::DoubleConstant(_) => Ok(AsmType::Double),
        TackyVal::Var(name) => match types.get(name).copied().unwrap_or(CType::Int) {
            CType::Char | CType::SChar | CType::UChar | CType::Bool => Ok(AsmType::Byte),
            CType::Short | CType::UShort => Ok(AsmType::Word),
            CType::Int | CType::UInt => Ok(AsmType::Longword),
            CType::Long | CType::ULong | CType::Pointer => Ok(AsmType::Quadword),
            CType::Int128 | CType::UInt128 => Ok(AsmType::Octword),
            CType::Float => Ok(AsmType::Float),
            CType::Double => Ok(AsmType::Double),
            CType::LongDouble => Ok(AsmType::LongDouble),
            CType::Void => Ok(AsmType::Longword),
            CType::Struct => Ok(AsmType::Quadword),
        },
    }
}

fn val_operand(
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<AsmOperand, String> {
    match val {
        TackyVal::Constant(c) => Ok(AsmOperand::Imm(*c)),
        TackyVal::Int128Constant(c) => Ok(AsmOperand::Imm(*c as i64)),
        TackyVal::UInt128Constant(c) => Ok(AsmOperand::Imm(*c as i64)),
        TackyVal::DoubleConstant(d) => Ok(AsmOperand::Imm(d.to_bits() as i64)),
        TackyVal::Var(name) => {
            if global_vars.contains(name) {
                Ok(AsmOperand::Data(name.clone()))
            } else {
                stack_slots
                    .get(name)
                    .copied()
                    .map(AsmOperand::Stack)
                    .ok_or_else(|| format!("AArch64 backend missing stack slot for {}", name))
            }
        }
    }
}

fn floating_return_operand(
    ty: AsmType,
    val: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<AsmOperand, String> {
    match (ty, val) {
        (AsmType::Float, TackyVal::DoubleConstant(d)) => {
            Ok(AsmOperand::Imm((*d as f32).to_bits() as i64))
        }
        (AsmType::Float, TackyVal::Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f32).to_bits() as i64))
        }
        (AsmType::Float, TackyVal::Int128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f32).to_bits() as i64))
        }
        (AsmType::Float, TackyVal::UInt128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f32).to_bits() as i64))
        }
        (AsmType::Double, TackyVal::DoubleConstant(d)) => Ok(AsmOperand::Imm(d.to_bits() as i64)),
        (AsmType::Double, TackyVal::Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f64).to_bits() as i64))
        }
        (AsmType::Double, TackyVal::Int128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f64).to_bits() as i64))
        }
        (AsmType::Double, TackyVal::UInt128Constant(c)) => {
            Ok(AsmOperand::Imm((*c as f64).to_bits() as i64))
        }
        _ => val_operand(val, stack_slots, global_vars),
    }
}

fn intern_long_double_const(pool: &mut Vec<(String, f64)>, value: f64) -> String {
    let bits = value.to_bits();
    if let Some((name, _)) = pool.iter().find(|(_, existing)| existing.to_bits() == bits) {
        return name.clone();
    }

    let name = format!("__aarch64_long_double_const_{}", pool.len());
    pool.push((name.clone(), value));
    name
}

fn rewrite_long_double_immediates(instructions: &mut [AsmInstr], pool: &mut Vec<(String, f64)>) {
    for instr in instructions {
        match instr {
            AsmInstr::Mov(AsmType::LongDouble, src, _)
            | AsmInstr::Binary(AsmType::LongDouble, _, src, _)
            | AsmInstr::Cmp(AsmType::LongDouble, src, _) => {
                if let AsmOperand::Imm(bits) = src {
                    let value = f64::from_bits(*bits as u64);
                    *src = AsmOperand::Data(intern_long_double_const(pool, value));
                }
            }
            _ => {}
        }
    }
}

fn collect_name(
    name: &str,
    vars: &mut Vec<String>,
    seen_vars: &mut HashSet<String>,
    global_vars: &HashSet<String>,
) {
    if !global_vars.contains(name) && seen_vars.insert(name.to_string()) {
        vars.push(name.to_string());
    }
}

fn collect_var(
    val: &TackyVal,
    vars: &mut Vec<String>,
    seen_vars: &mut HashSet<String>,
    global_vars: &HashSet<String>,
) {
    if let TackyVal::Var(name) = val {
        collect_name(name, vars, seen_vars, global_vars);
    }
}

fn val_ctype(val: &TackyVal, types: &HashMap<String, CType>) -> Option<CType> {
    match val {
        TackyVal::Var(name) => types.get(name).copied(),
        TackyVal::Constant(_) => Some(CType::Int),
        TackyVal::Int128Constant(_) => Some(CType::Int128),
        TackyVal::UInt128Constant(_) => Some(CType::UInt128),
        TackyVal::DoubleConstant(_) => Some(CType::Double),
    }
}

fn group_register_needs(is_sse: &[bool]) -> (usize, usize) {
    let fp_needed = is_sse.iter().filter(|&&is_fp| is_fp).count();
    (is_sse.len() - fp_needed, fp_needed)
}

fn data_operand_with_offset(name: &str, offset: i32) -> AsmOperand {
    if offset == 0 {
        AsmOperand::Data(name.to_string())
    } else {
        AsmOperand::Data(format!("{}+{}", name, offset))
    }
}

struct TlsNameOffset {
    base: String,
    offset: i32,
}

fn tls_name_offset(name: &str, tls_vars: &HashSet<String>) -> Option<TlsNameOffset> {
    if tls_vars.contains(name) {
        return Some(TlsNameOffset {
            base: name.to_string(),
            offset: 0,
        });
    }
    let (base, offset) = name.rsplit_once('+')?;
    if tls_vars.contains(base) {
        let offset = offset.parse().ok()?;
        Some(TlsNameOffset {
            base: base.to_string(),
            offset,
        })
    } else {
        None
    }
}

fn rewrite_tls_operand(op: &mut AsmOperand, tls_vars: &HashSet<String>) {
    if let AsmOperand::Data(name) = op {
        if let Some(tls_offset) = tls_name_offset(name, tls_vars) {
            *op = AsmOperand::TlsData(tls_offset.base, tls_offset.offset);
        }
    }
}

fn rewrite_tls_operands(func: &mut AsmFunction, tls_vars: &HashSet<String>) {
    for instr in &mut func.instructions {
        match instr {
            AsmInstr::Mov(_, src, dst)
            | AsmInstr::Movsx(_, _, src, dst)
            | AsmInstr::MovZeroExtend(_, _, src, dst)
            | AsmInstr::Binary(_, _, src, dst)
            | AsmInstr::Cmp(_, src, dst)
            | AsmInstr::Lea(src, dst)
            | AsmInstr::Cvtsi2sd(_, src, dst)
            | AsmInstr::Cvttsd2si(_, src, dst)
            | AsmInstr::Cvtsi2ss(_, src, dst)
            | AsmInstr::Cvttss2si(_, src, dst)
            | AsmInstr::Cvtss2sd(src, dst)
            | AsmInstr::Cvtsd2ss(src, dst)
            | AsmInstr::AArch64UIntToDouble(_, src, dst)
            | AsmInstr::AArch64DoubleToUInt(_, src, dst)
            | AsmInstr::AArch64UIntToFloat(_, src, dst)
            | AsmInstr::AArch64FloatToUInt(_, src, dst)
            | AsmInstr::AArch64FloatToDouble(src, dst)
            | AsmInstr::AArch64DoubleToFloat(src, dst) => {
                rewrite_tls_operand(src, tls_vars);
                rewrite_tls_operand(dst, tls_vars);
            }
            AsmInstr::AtomicRmw(_, _, _, dst)
            | AsmInstr::AtomicExchange(_, dst)
            | AsmInstr::AtomicCompareExchange(_, dst)
            | AsmInstr::AtomicCompareSwap(_, _, dst) => rewrite_tls_operand(dst, tls_vars),
            AsmInstr::Unary(_, _, op)
            | AsmInstr::Idiv(_, op)
            | AsmInstr::Div(_, op)
            | AsmInstr::SetCC(_, op)
            | AsmInstr::Push(op)
            | AsmInstr::LoadIndirect(_, _, op)
            | AsmInstr::StoreIndirect(_, op, _) => rewrite_tls_operand(op, tls_vars),
            _ => {}
        }
    }
}

fn stack_or_data_operand(
    name: &str,
    offset: i32,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<AsmOperand, String> {
    if global_vars.contains(name) {
        Ok(data_operand_with_offset(name, offset))
    } else {
        stack_slots
            .get(name)
            .copied()
            .map(|base| AsmOperand::Stack(base + offset))
            .ok_or_else(|| format!("AArch64 backend missing stack slot for {}", name))
    }
}

fn emit_copy_pointer_to_outgoing_arg(
    instructions: &mut Vec<AsmInstr>,
    src_ptr: AsmOperand,
    size: usize,
    dst_start: i32,
    outgoing_bytes: i32,
) {
    instructions.push(AsmInstr::AArch64LoadAdjusted(
        AsmType::Quadword,
        src_ptr,
        Reg::R11,
        outgoing_bytes,
    ));
    instructions.push(AsmInstr::CopyToStackArg {
        src_ptr: AsmOperand::Reg(Reg::R11),
        dst_offset: dst_start,
        size,
    });
}

fn emit_copy_incoming_arg_to_aggregate(
    instructions: &mut Vec<AsmInstr>,
    src_start: i32,
    dst_name: &str,
    size: usize,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    instructions.push(AsmInstr::CopyFromStackArg {
        src_offset: src_start,
        dst: stack_or_data_operand(dst_name, 0, stack_slots, global_vars)?,
        size,
    });
    Ok(())
}

fn aggregate_size(
    name: &str,
    array_sizes: &HashMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Option<usize> {
    array_sizes.get(name).copied().or_else(|| {
        var_struct_tags
            .get(name)
            .and_then(|tag| struct_defs.get(tag))
            .map(|def| def.size)
    })
}

fn struct_classes_for_val(
    val: &TackyVal,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Option<Vec<ParamClass>> {
    let TackyVal::Var(name) = val else {
        return None;
    };
    var_struct_tags
        .get(name)
        .and_then(|tag| struct_defs.get(tag))
        .map(|def| def.classify_with(struct_defs))
}

fn struct_size_for_val(
    val: &TackyVal,
    array_sizes: &HashMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
) -> Option<usize> {
    let TackyVal::Var(name) = val else {
        return None;
    };
    aggregate_size(name, array_sizes, var_struct_tags, struct_defs)
}

fn copy_bytes(
    instructions: &mut Vec<AsmInstr>,
    src_name: &str,
    dst_name: &str,
    size: usize,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let mut offset = 0usize;
    while offset + 8 <= size {
        let byte_offset = i32::try_from(offset)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", src_name))?;
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            stack_or_data_operand(src_name, byte_offset, stack_slots, global_vars)?,
            AsmOperand::Reg(Reg::R10),
        ));
        instructions.push(AsmInstr::Mov(
            AsmType::Quadword,
            AsmOperand::Reg(Reg::R10),
            stack_or_data_operand(dst_name, byte_offset, stack_slots, global_vars)?,
        ));
        offset += 8;
    }
    while offset + 4 <= size {
        let byte_offset = i32::try_from(offset)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", src_name))?;
        instructions.push(AsmInstr::Mov(
            AsmType::Longword,
            stack_or_data_operand(src_name, byte_offset, stack_slots, global_vars)?,
            AsmOperand::Reg(Reg::R10),
        ));
        instructions.push(AsmInstr::Mov(
            AsmType::Longword,
            AsmOperand::Reg(Reg::R10),
            stack_or_data_operand(dst_name, byte_offset, stack_slots, global_vars)?,
        ));
        offset += 4;
    }
    while offset < size {
        let byte_offset = i32::try_from(offset)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", src_name))?;
        instructions.push(AsmInstr::Mov(
            AsmType::Byte,
            stack_or_data_operand(src_name, byte_offset, stack_slots, global_vars)?,
            AsmOperand::Reg(Reg::R10),
        ));
        instructions.push(AsmInstr::Mov(
            AsmType::Byte,
            AsmOperand::Reg(Reg::R10),
            stack_or_data_operand(dst_name, byte_offset, stack_slots, global_vars)?,
        ));
        offset += 1;
    }
    Ok(())
}

fn move_struct_to_return_regs(
    instructions: &mut Vec<AsmInstr>,
    val: &TackyVal,
    classes: &[ParamClass],
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let TackyVal::Var(name) = val else {
        return Err("AArch64 backend can only return struct variables".to_string());
    };
    let gp_regs = [Reg::AX, Reg::DX];
    let fp_regs = [XmmReg::XMM0, XmmReg::XMM1];
    let mut gp_idx = 0usize;
    let mut fp_idx = 0usize;
    for (chunk_idx, class) in classes.iter().enumerate() {
        let offset = i32::try_from(chunk_idx * 8)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", name))?;
        match class {
            ParamClass::Integer if gp_idx < gp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                    AsmOperand::Reg(gp_regs[gp_idx]),
                ));
                gp_idx += 1;
            }
            ParamClass::Sse if fp_idx < fp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                    AsmOperand::Xmm(fp_regs[fp_idx]),
                ));
                fp_idx += 1;
            }
            ParamClass::Memory => {
                return Err(
                    "AArch64 backend does not return memory-class structs in registers yet"
                        .to_string(),
                );
            }
            other => {
                return Err(format!(
                    "AArch64 backend unsupported struct return class: {:?}",
                    other
                ));
            }
        }
    }
    Ok(())
}

fn move_return_regs_to_struct(
    instructions: &mut Vec<AsmInstr>,
    dst: &TackyVal,
    classes: &[ParamClass],
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let TackyVal::Var(name) = dst else {
        return Err("AArch64 backend can only store struct returns into variables".to_string());
    };
    let gp_regs = [Reg::AX, Reg::DX];
    let fp_regs = [XmmReg::XMM0, XmmReg::XMM1];
    let mut gp_idx = 0usize;
    let mut fp_idx = 0usize;
    for (chunk_idx, class) in classes.iter().enumerate() {
        let offset = i32::try_from(chunk_idx * 8)
            .map_err(|_| format!("AArch64 backend aggregate offset too large: {}", name))?;
        match class {
            ParamClass::Integer if gp_idx < gp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Reg(gp_regs[gp_idx]),
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                ));
                gp_idx += 1;
            }
            ParamClass::Sse if fp_idx < fp_regs.len() => {
                instructions.push(AsmInstr::Mov(
                    AsmType::Double,
                    AsmOperand::Xmm(fp_regs[fp_idx]),
                    stack_or_data_operand(name, offset, stack_slots, global_vars)?,
                ));
                fp_idx += 1;
            }
            ParamClass::Memory => {
                return Err(
                    "AArch64 backend does not receive memory-class structs in registers yet"
                        .to_string(),
                );
            }
            other => {
                return Err(format!(
                    "AArch64 backend unsupported struct return class: {:?}",
                    other
                ));
            }
        }
    }
    Ok(())
}

fn collect_stack_slots(
    function: &TackyFunction,
    types: &HashMap<String, CType>,
    array_sizes: &HashMap<String, usize>,
    var_struct_tags: &HashMap<String, String>,
    struct_defs: &HashMap<String, StructDef>,
    global_vars: &HashSet<String>,
    alignments: &HashMap<String, usize>,
) -> Result<StackLayout, String> {
    let mut vars = Vec::new();
    let mut seen_vars = HashSet::new();
    for param in &function.params {
        collect_name(param, &mut vars, &mut seen_vars, global_vars);
    }
    for (_, name, _) in &function.memory_param_blocks {
        collect_name(name, &mut vars, &mut seen_vars, global_vars);
    }

    for instr in &function.body {
        match instr {
            TackyInstr::Return(val) => collect_var(val, &mut vars, &mut seen_vars, global_vars),
            TackyInstr::Unary { src, dst, .. } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Binary {
                left, right, dst, ..
            } => {
                collect_var(left, &mut vars, &mut seen_vars, global_vars);
                collect_var(right, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Copy { src, dst }
            | TackyInstr::SignExtend { src, dst }
            | TackyInstr::ZeroExtend { src, dst }
            | TackyInstr::Truncate { src, dst }
            | TackyInstr::IntToDouble { src, dst }
            | TackyInstr::IntToFloat { src, dst }
            | TackyInstr::DoubleToInt { src, dst }
            | TackyInstr::FloatToInt { src, dst }
            | TackyInstr::UIntToDouble { src, dst }
            | TackyInstr::UIntToFloat { src, dst }
            | TackyInstr::DoubleToUInt { src, dst }
            | TackyInstr::FloatToUInt { src, dst }
            | TackyInstr::FloatToDouble { src, dst }
            | TackyInstr::DoubleToFloat { src, dst } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::JumpIfZero(val, _)
            | TackyInstr::JumpIfNotZero(val, _)
            | TackyInstr::JumpIndirect(val) => {
                collect_var(val, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::LoadLabelAddress(_, dst) => {
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::FrameAddress { dst } => {
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::BuiltinSetjmp { buf, dst, .. } => {
                collect_var(buf, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::BuiltinLongjmp { buf, value } => {
                collect_var(buf, &mut vars, &mut seen_vars, global_vars);
                collect_var(value, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::FunCall {
                name,
                args,
                dst,
                indirect,
                ..
            } => {
                if *indirect {
                    collect_name(name, &mut vars, &mut seen_vars, global_vars);
                }
                for arg in args {
                    collect_var(arg, &mut vars, &mut seen_vars, global_vars);
                }
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::VaStart { dst } => {
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::GetAddress { src, dst } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Load { src_ptr, dst } => {
                collect_var(src_ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Store { src, dst_ptr } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst_ptr, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicFetch { ptr, arg, dst, .. } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(arg, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicExchange { ptr, value, dst } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(value, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicCompareExchange {
                ptr,
                expected,
                desired,
                dst,
            } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(expected, &mut vars, &mut seen_vars, global_vars);
                collect_var(desired, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                dst,
                ..
            } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(expected, &mut vars, &mut seen_vars, global_vars);
                collect_var(desired, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::AddPtr {
                ptr, index, dst, ..
            } => {
                collect_var(ptr, &mut vars, &mut seen_vars, global_vars);
                collect_var(index, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::CopyToOffset { src, dst_name, .. } => {
                collect_var(src, &mut vars, &mut seen_vars, global_vars);
                collect_name(dst_name, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::CopyFromOffset { src_name, dst, .. } => {
                collect_name(src_name, &mut vars, &mut seen_vars, global_vars);
                collect_var(dst, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::CopyStruct { src_name, dst_name } => {
                collect_name(src_name, &mut vars, &mut seen_vars, global_vars);
                collect_name(dst_name, &mut vars, &mut seen_vars, global_vars);
            }
            TackyInstr::Jump(_)
            | TackyInstr::NonlocalJump(_)
            | TackyInstr::Label(_)
            | TackyInstr::Nop
            | TackyInstr::Unreachable
            | TackyInstr::AtomicFence => {}
        }
    }

    let mut slots = HashMap::new();
    let mut large_locals = Vec::new();
    let mut offset = 0i32;
    for var in vars {
        let size =
            if let Some(size) = aggregate_size(&var, array_sizes, var_struct_tags, struct_defs) {
                if size > i32::MAX as usize {
                    offset = align_to(offset, STACK_SLOT_SIZE);
                    let base_slot = offset;
                    slots.insert(var.clone(), base_slot);
                    offset += STACK_SLOT_SIZE;
                    large_locals.push(LargeLocal {
                        name: var,
                        size,
                        base_slot,
                    });
                    continue;
                } else {
                    size as i32
                }
            } else {
                match types.get(&var).copied().unwrap_or(CType::Int) {
                    CType::Char | CType::SChar | CType::UChar | CType::Bool => 1,
                    CType::Short | CType::UShort => 2,
                    CType::Int | CType::UInt => 4,
                    CType::Float => 4,
                    CType::Long | CType::ULong | CType::Pointer => 8,
                    CType::Int128 | CType::UInt128 | CType::LongDouble => 16,
                    CType::Void => 4,
                    CType::Double => 8,
                    CType::Struct => {
                        return Err(format!(
                            "AArch64 backend missing aggregate size for local {}",
                            var
                        ))
                    }
                }
            };
        let align = if let Some(&decl_align) = alignments.get(&var) {
            i32::try_from(decl_align)
                .map_err(|_| format!("AArch64 backend alignment too large for {}", var))?
        } else if size >= STACK_SLOT_SIZE {
            STACK_SLOT_SIZE
        } else {
            MIN_STACK_SLOT_SIZE
        };
        offset = align_to(offset, align);
        slots.insert(var, offset);
        offset += size.max(MIN_STACK_SLOT_SIZE);
    }

    Ok(StackLayout {
        slots,
        local_stack_size: align_to(offset, STACK_ALIGNMENT),
        large_locals,
    })
}

fn convert_binary_op(op: &TackyBinaryOp) -> Result<AsmBinaryOp, String> {
    match op {
        TackyBinaryOp::Add => Ok(AsmBinaryOp::Add),
        TackyBinaryOp::Sub => Ok(AsmBinaryOp::Sub),
        TackyBinaryOp::Mul => Ok(AsmBinaryOp::Mul),
        TackyBinaryOp::ShiftLeft => Ok(AsmBinaryOp::Sal),
        TackyBinaryOp::BitwiseAnd => Ok(AsmBinaryOp::And),
        TackyBinaryOp::BitwiseNand => Ok(AsmBinaryOp::Nand),
        TackyBinaryOp::BitwiseOr => Ok(AsmBinaryOp::Or),
        TackyBinaryOp::BitwiseXor => Ok(AsmBinaryOp::Xor),
        _ => Err(format!(
            "AArch64 backend does not support binary operator yet: {:?}",
            op
        )),
    }
}

fn long_double_helper(op: &TackyBinaryOp) -> Option<&'static str> {
    match op {
        TackyBinaryOp::Add => Some("__addtf3"),
        TackyBinaryOp::Sub => Some("__subtf3"),
        TackyBinaryOp::Mul => Some("__multf3"),
        TackyBinaryOp::Div => Some("__divtf3"),
        _ => None,
    }
}

struct LongDoubleComparison {
    helper: &'static str,
    condition: CondCode,
}

fn long_double_comparison_helper(op: &TackyBinaryOp) -> Option<LongDoubleComparison> {
    match op {
        TackyBinaryOp::Equal => Some(LongDoubleComparison {
            helper: "__eqtf2",
            condition: CondCode::E,
        }),
        TackyBinaryOp::NotEqual => Some(LongDoubleComparison {
            helper: "__netf2",
            condition: CondCode::NE,
        }),
        TackyBinaryOp::LessThan => Some(LongDoubleComparison {
            helper: "__lttf2",
            condition: CondCode::L,
        }),
        TackyBinaryOp::LessEqual => Some(LongDoubleComparison {
            helper: "__letf2",
            condition: CondCode::LE,
        }),
        TackyBinaryOp::GreaterThan => Some(LongDoubleComparison {
            helper: "__gttf2",
            condition: CondCode::G,
        }),
        TackyBinaryOp::GreaterEqual => Some(LongDoubleComparison {
            helper: "__getf2",
            condition: CondCode::GE,
        }),
        _ => None,
    }
}

fn emit_long_double_helper_call(
    instructions: &mut Vec<AsmInstr>,
    helper: &str,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(left, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM0),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(right, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM1),
    ));
    instructions.push(AsmInstr::Call(helper.to_string(), 0, 2, false, false));
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        AsmOperand::Xmm(XmmReg::XMM0),
        val_operand(dst, stack_slots, global_vars)?,
    ));
    Ok(())
}

fn emit_long_double_comparison(
    instructions: &mut Vec<AsmInstr>,
    comparison: LongDoubleComparison,
    left: &TackyVal,
    right: &TackyVal,
    dst: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(left, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM0),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(right, stack_slots, global_vars)?,
        AsmOperand::Xmm(XmmReg::XMM1),
    ));
    instructions.push(AsmInstr::Call(
        comparison.helper.to_string(),
        0,
        2,
        false,
        false,
    ));
    instructions.push(AsmInstr::Cmp(
        AsmType::Longword,
        AsmOperand::Imm(0),
        AsmOperand::Reg(Reg::AX),
    ));
    instructions.push(AsmInstr::SetCC(
        comparison.condition,
        val_operand(dst, stack_slots, global_vars)?,
    ));
    Ok(())
}

fn emit_long_double_negate(
    instructions: &mut Vec<AsmInstr>,
    src: &TackyVal,
    dst: &TackyVal,
    stack_slots: &HashMap<String, i32>,
    global_vars: &HashSet<String>,
) -> Result<(), String> {
    let dst_op = val_operand(dst, stack_slots, global_vars)?;
    instructions.push(AsmInstr::Mov(
        AsmType::LongDouble,
        val_operand(src, stack_slots, global_vars)?,
        dst_op.clone(),
    ));
    let sign_byte = byte_offset_operand(&dst_op, 15)?;
    instructions.push(AsmInstr::Mov(
        AsmType::Byte,
        sign_byte.clone(),
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Binary(
        AsmType::Byte,
        AsmBinaryOp::Xor,
        AsmOperand::Imm(0x80),
        AsmOperand::Reg(Reg::R10),
    ));
    instructions.push(AsmInstr::Mov(
        AsmType::Byte,
        AsmOperand::Reg(Reg::R10),
        sign_byte,
    ));
    Ok(())
}

fn convert_comparison_op(op: &TackyBinaryOp, is_unsigned: bool) -> Option<CondCode> {
    match (op, is_unsigned) {
        (TackyBinaryOp::Equal, _) => Some(CondCode::E),
        (TackyBinaryOp::NotEqual, _) => Some(CondCode::NE),
        (TackyBinaryOp::LessThan, false) => Some(CondCode::L),
        (TackyBinaryOp::LessEqual, false) => Some(CondCode::LE),
        (TackyBinaryOp::GreaterThan, false) => Some(CondCode::G),
        (TackyBinaryOp::GreaterEqual, false) => Some(CondCode::GE),
        (TackyBinaryOp::LessThan, true) => Some(CondCode::B),
        (TackyBinaryOp::LessEqual, true) => Some(CondCode::BE),
        (TackyBinaryOp::GreaterThan, true) => Some(CondCode::A),
        (TackyBinaryOp::GreaterEqual, true) => Some(CondCode::AE),
        _ => None,
    }
}

fn is_unsigned_val(val: &TackyVal, types: &HashMap<String, CType>) -> bool {
    match val {
        TackyVal::Var(name) => types
            .get(name)
            .copied()
            .is_some_and(|ctype| !ctype.is_signed()),
        _ => false,
    }
}

fn is_unsigned_comparison_val(val: &TackyVal, types: &HashMap<String, CType>) -> bool {
    match val {
        TackyVal::Var(name) => types
            .get(name)
            .copied()
            .is_some_and(|ctype| ctype != CType::Double && !ctype.is_signed()),
        _ => false,
    }
}

fn convert_function(
    function: &TackyFunction,
    target: &Target,
    program: &TackyProgram,
    long_double_consts: &mut Vec<(String, f64)>,
) -> Result<AsmFunction, String> {
    let types = &program.symbol_types;
    let array_sizes = &program.array_sizes;
    let var_struct_tags = &program.var_struct_tags;
    let struct_defs = &program.struct_defs;
    let global_vars = &program.global_vars;
    let alignments = &program.symbol_alignments;
    let stack_layout = collect_stack_slots(
        function,
        types,
        array_sizes,
        var_struct_tags,
        struct_defs,
        global_vars,
        alignments,
    )?;
    let stack_slots = stack_layout.slots;
    let stack_size = stack_layout.local_stack_size;
    let saves_link_register = function.body.iter().any(|instr| match instr {
        TackyInstr::FunCall { .. } => true,
        TackyInstr::Binary {
            op,
            left,
            right,
            dst,
        } => {
            (matches!(asm_type_for_val(dst, types), Ok(AsmType::LongDouble))
                && long_double_helper(op).is_some()
                || (long_double_comparison_helper(op).is_some()
                    && (matches!(asm_type_for_val(left, types), Ok(AsmType::LongDouble))
                        || matches!(asm_type_for_val(right, types), Ok(AsmType::LongDouble)))))
                || matches!(
                    (op, asm_type_for_val(dst, types)),
                    (
                        TackyBinaryOp::Div | TackyBinaryOp::Mod | TackyBinaryOp::Mul,
                        Ok(AsmType::Octword)
                    )
                )
        }
        _ => false,
    });
    let frame_size = compute_frame_size(stack_size, saves_link_register);
    let link_register_offset =
        saves_link_register.then(|| compute_link_register_offset(frame_size));
    let mut large_local_offsets = HashMap::new();
    let mut large_stack_size = 0i64;
    for local in &stack_layout.large_locals {
        let base_offset = frame_size as i64 + large_stack_size;
        large_local_offsets.insert(local.name.clone(), (local.base_slot, base_offset));
        let aligned_size = align_usize_to(local.size, LARGE_LOCAL_ALIGNMENT)?;
        let aligned_size = i64::try_from(aligned_size)
            .map_err(|_| format!("AArch64 backend large local too large: {}", local.name))?;
        large_stack_size = large_stack_size
            .checked_add(aligned_size)
            .ok_or_else(|| "AArch64 backend large stack allocation overflow".to_string())?;
    }
    large_stack_size = align_i64_to(large_stack_size, STACK_ALIGNMENT as i64)?;
    if large_stack_size > 0
        && (!function.stack_params.is_empty()
            || !function.memory_param_blocks.is_empty()
            || function.params.len() > ARG_REGS.len()
            || function
                .body
                .iter()
                .any(|instr| matches!(instr, TackyInstr::VaStart { .. })))
    {
        return Err(format!(
            "AArch64 backend does not yet support large local arrays with incoming stack arguments in {}",
            function.name
        ));
    }
    let mut instructions = Vec::new();
    if large_stack_size > 0 {
        instructions.push(AsmInstr::AArch64AllocateLargeStack(large_stack_size));
    }
    if frame_size > 0 {
        instructions.push(AsmInstr::AllocateStack(frame_size));
    }
    if let Some(offset) = link_register_offset {
        instructions.push(AsmInstr::AArch64SaveLink(offset));
    }
    for local in &stack_layout.large_locals {
        let Some((base_slot, base_offset)) = large_local_offsets.get(&local.name).copied() else {
            continue;
        };
        instructions.push(AsmInstr::AArch64StoreLargeLocalBase {
            base_offset,
            dst_offset: base_slot,
        });
    }
    let param_groups: HashMap<usize, (usize, Vec<bool>)> = function
        .struct_param_groups
        .iter()
        .map(|(start, count, is_sse)| (*start, (*count, is_sse.clone())))
        .collect();
    let memory_param_blocks: HashMap<usize, (&String, usize)> = function
        .memory_param_blocks
        .iter()
        .map(|(index, name, size)| (*index, (name, *size)))
        .collect();
    let mut gp_param_count = 0usize;
    let mut fp_param_count = 0usize;
    let mut stack_param_count = 0usize;
    let mut param_index = 0usize;
    while param_index < function.params.len() {
        if let Some((dst_name, size)) = memory_param_blocks.get(&param_index).copied() {
            let src_start = stack_arg_offset(frame_size, stack_param_count);
            emit_copy_incoming_arg_to_aggregate(
                &mut instructions,
                src_start,
                dst_name,
                size,
                &stack_slots,
                global_vars,
            )?;
            stack_param_count += size.div_ceil(STACK_SLOT_SIZE as usize);
            param_index += 1;
            continue;
        }
        if let Some((count, is_sse)) = param_groups.get(&param_index) {
            let (gp_needed, fp_needed) = group_register_needs(is_sse);
            let fits_registers = gp_param_count + gp_needed <= ARG_REGS.len()
                && fp_param_count + fp_needed <= FP_ARG_REGS.len();
            for (group_offset, is_fp) in is_sse.iter().copied().enumerate().take(*count) {
                let param = &function.params[param_index + group_offset];
                let ty = asm_type_for_val(&TackyVal::Var(param.clone()), types)?;
                let src = if fits_registers {
                    if is_fp {
                        let src = AsmOperand::Xmm(FP_ARG_REGS[fp_param_count]);
                        fp_param_count += 1;
                        src
                    } else {
                        let src = AsmOperand::Reg(ARG_REGS[gp_param_count]);
                        gp_param_count += 1;
                        src
                    }
                } else {
                    let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
                    stack_param_count += 1;
                    src
                };
                instructions.push(AsmInstr::Mov(
                    ty,
                    src,
                    val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?,
                ));
            }
            param_index += *count;
            continue;
        }

        let param = &function.params[param_index];
        let ty = asm_type_for_val(&TackyVal::Var(param.clone()), types)?;
        if ty == AsmType::Octword
            && !function.stack_params.contains(param)
            && gp_param_count + 1 < ARG_REGS.len()
        {
            let dst = val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?;
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(ARG_REGS[gp_param_count]),
                low64_operand(&dst)?,
            ));
            instructions.push(AsmInstr::Mov(
                AsmType::Quadword,
                AsmOperand::Reg(ARG_REGS[gp_param_count + 1]),
                high64_operand(&dst)?,
            ));
            gp_param_count += 2;
            param_index += 1;
            continue;
        }
        let src = if function.stack_params.contains(param) {
            if ty == AsmType::Octword {
                let dst = val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?;
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count)),
                    low64_operand(&dst)?,
                ));
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count + 1)),
                    high64_operand(&dst)?,
                ));
                stack_param_count += 2;
                param_index += 1;
                continue;
            }
            let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
            stack_param_count += 1;
            src
        } else if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
            if fp_param_count < FP_ARG_REGS.len() {
                let src = AsmOperand::Xmm(FP_ARG_REGS[fp_param_count]);
                fp_param_count += 1;
                src
            } else {
                let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
                stack_param_count += if ty == AsmType::LongDouble { 2 } else { 1 };
                src
            }
        } else if gp_param_count < ARG_REGS.len() {
            let src = AsmOperand::Reg(ARG_REGS[gp_param_count]);
            gp_param_count += 1;
            src
        } else {
            if matches!(ty, AsmType::Octword | AsmType::LongDouble) {
                let dst = val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?;
                if ty == AsmType::LongDouble {
                    instructions.push(AsmInstr::Mov(
                        AsmType::LongDouble,
                        AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count)),
                        dst,
                    ));
                } else {
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count)),
                        low64_operand(&dst)?,
                    ));
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count + 1)),
                        high64_operand(&dst)?,
                    ));
                }
                stack_param_count += 2;
                param_index += 1;
                continue;
            }
            let src = AsmOperand::Stack(stack_arg_offset(frame_size, stack_param_count));
            stack_param_count += 1;
            src
        };
        instructions.push(AsmInstr::Mov(
            ty,
            src,
            val_operand(&TackyVal::Var(param.clone()), &stack_slots, global_vars)?,
        ));
        param_index += 1;
    }
    let va_start_stack_offset = stack_arg_offset(frame_size, stack_param_count);
    let i128_ctx = Aarch64I128Context {
        function_name: &function.name,
        types,
        stack_slots: &stack_slots,
        global_vars,
    };

    for instr in &function.body {
        match instr {
            TackyInstr::Unreachable => {
                instructions.push(AsmInstr::Unreachable);
            }
            TackyInstr::Nop => {}
            TackyInstr::AtomicFence => {
                instructions.push(AsmInstr::AtomicFence);
            }
            TackyInstr::AtomicFetch {
                op,
                ptr,
                arg,
                return_old,
                dst,
            } => {
                let ty = asm_type_for_val(dst, types)?;
                if matches!(ty, AsmType::Float | AsmType::Double) {
                    return Err("AArch64 backend cannot atomic-fetch floating values".to_string());
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(arg, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicRmw(
                    ty,
                    convert_binary_op(op)?,
                    *return_old,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AtomicExchange { ptr, value, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                if matches!(ty, AsmType::Float | AsmType::Double) {
                    return Err(
                        "AArch64 backend cannot atomic-exchange floating values".to_string()
                    );
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(value, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicExchange(
                    ty,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AtomicCompareExchange {
                ptr,
                expected,
                desired,
                dst,
            } => {
                let desired_ty = asm_type_for_val(desired, types)?;
                if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                    return Err(
                        "AArch64 backend cannot atomic-compare-exchange floating values"
                            .to_string(),
                    );
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(expected, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R12),
                ));
                instructions.push(AsmInstr::Mov(
                    desired_ty,
                    val_operand(desired, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicCompareExchange(
                    desired_ty,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AtomicCompareSwap {
                ptr,
                expected,
                desired,
                return_old,
                dst,
            } => {
                let desired_ty = asm_type_for_val(desired, types)?;
                if matches!(desired_ty, AsmType::Float | AsmType::Double) {
                    return Err(
                        "AArch64 backend cannot sync compare-and-swap floating values".to_string(),
                    );
                }
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::Mov(
                    desired_ty,
                    val_operand(expected, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R12),
                ));
                instructions.push(AsmInstr::Mov(
                    desired_ty,
                    val_operand(desired, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R10),
                ));
                instructions.push(AsmInstr::AtomicCompareSwap(
                    desired_ty,
                    *return_old,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Return(val) => {
                if val_ctype(val, types) == Some(CType::Struct) {
                    let classes = struct_classes_for_val(val, var_struct_tags, struct_defs)
                        .ok_or_else(|| {
                            "AArch64 backend missing struct class for return value".to_string()
                        })?;
                    if struct_size_for_val(val, array_sizes, var_struct_tags, struct_defs)
                        .is_some_and(|size| size > 16)
                    {
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            val_operand(val, &stack_slots, global_vars)?,
                            AsmOperand::Reg(Reg::AX),
                        ));
                    } else {
                        move_struct_to_return_regs(
                            &mut instructions,
                            val,
                            &classes,
                            &stack_slots,
                            global_vars,
                        )?;
                    }
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    continue;
                }
                let ty = match val {
                    TackyVal::Var(_) => asm_type_for_val(val, types)?,
                    _ => function.return_type.into(),
                };
                if ty == AsmType::Octword {
                    emit_i128_return(&mut instructions, val, &stack_slots, global_vars)?;
                    emit_epilogue(
                        &mut instructions,
                        frame_size,
                        large_stack_size,
                        link_register_offset,
                    );
                    continue;
                }
                let ret_dst =
                    if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
                        AsmOperand::Xmm(XmmReg::XMM0)
                    } else {
                        AsmOperand::Reg(Reg::AX)
                    };
                let src = if matches!(ty, AsmType::Float | AsmType::Double) {
                    floating_return_operand(ty, val, &stack_slots, global_vars)?
                } else {
                    val_operand(val, &stack_slots, global_vars)?
                };
                instructions.push(AsmInstr::Mov(ty, src, ret_dst));
                emit_epilogue(
                    &mut instructions,
                    frame_size,
                    large_stack_size,
                    link_register_offset,
                );
            }
            TackyInstr::Copy { src, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                if ty == AsmType::Octword {
                    emit_i128_copy(&mut instructions, src, dst, &stack_slots, global_vars)?;
                    continue;
                }
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::SignExtend { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                let dst_ty = asm_type_for_val(dst, types)?;
                if dst_ty == AsmType::Octword {
                    let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                    let dst_low = low64_operand(&dst_op)?;
                    let dst_high = high64_operand(&dst_op)?;
                    match src {
                        TackyVal::Constant(c) => {
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Imm(*c),
                                dst_low,
                            ));
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Imm(if *c < 0 { -1 } else { 0 }),
                                dst_high,
                            ));
                        }
                        _ => {
                            if src_ty == AsmType::Quadword {
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    dst_low.clone(),
                                ));
                            } else {
                                instructions.push(AsmInstr::Movsx(
                                    src_ty,
                                    AsmType::Quadword,
                                    val_operand(src, &stack_slots, global_vars)?,
                                    dst_low.clone(),
                                ));
                            }
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                dst_low.clone(),
                                dst_high.clone(),
                            ));
                            instructions.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Sar,
                                AsmOperand::Imm(63),
                                dst_high,
                            ));
                        }
                    }
                    continue;
                }
                match src {
                    TackyVal::Constant(c) if dst_ty != AsmType::Byte => {
                        instructions.push(AsmInstr::Mov(
                            dst_ty,
                            AsmOperand::Imm(*c),
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                    _ => {
                        instructions.push(AsmInstr::Movsx(
                            src_ty,
                            dst_ty,
                            val_operand(src, &stack_slots, global_vars)?,
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                }
            }
            TackyInstr::ZeroExtend { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                let dst_ty = asm_type_for_val(dst, types)?;
                if dst_ty == AsmType::Octword {
                    let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                    let dst_low = low64_operand(&dst_op)?;
                    let dst_high = high64_operand(&dst_op)?;
                    if src_ty == AsmType::Quadword {
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            val_operand(src, &stack_slots, global_vars)?,
                            dst_low,
                        ));
                    } else {
                        instructions.push(AsmInstr::MovZeroExtend(
                            src_ty,
                            AsmType::Quadword,
                            val_operand(src, &stack_slots, global_vars)?,
                            dst_low,
                        ));
                    }
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Imm(0),
                        dst_high,
                    ));
                    continue;
                }
                match src {
                    TackyVal::Constant(c) if dst_ty != AsmType::Byte => {
                        instructions.push(AsmInstr::Mov(
                            dst_ty,
                            AsmOperand::Imm(*c),
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                    _ => {
                        instructions.push(AsmInstr::MovZeroExtend(
                            src_ty,
                            dst_ty,
                            val_operand(src, &stack_slots, global_vars)?,
                            val_operand(dst, &stack_slots, global_vars)?,
                        ));
                    }
                }
            }
            TackyInstr::Truncate { src, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                if asm_type_for_val(src, types)? == AsmType::Octword {
                    instructions.push(AsmInstr::Mov(
                        ty,
                        low64_operand(&val_operand(src, &stack_slots, global_vars)?)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                    continue;
                }
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::IntToDouble { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                if matches!(src_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Movsx(
                        src_ty,
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Cvtsi2sd(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvtsi2sd(
                        src_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::IntToFloat { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                if matches!(src_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Movsx(
                        src_ty,
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Cvtsi2ss(
                        AsmType::Longword,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvtsi2ss(
                        src_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::UIntToDouble { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::AArch64UIntToDouble(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::UIntToFloat { src, dst } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::AArch64UIntToFloat(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::DoubleToInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Cvttsd2si(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvttsd2si(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::FloatToInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::Cvttss2si(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Cvttss2si(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::DoubleToUInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::AArch64DoubleToUInt(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::AArch64DoubleToUInt(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::FloatToUInt { src, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                if matches!(dst_ty, AsmType::Byte | AsmType::Word) {
                    instructions.push(AsmInstr::AArch64FloatToUInt(
                        AsmType::Longword,
                        val_operand(src, &stack_slots, global_vars)?,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        dst_ty,
                        AsmOperand::Reg(Reg::R10),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::AArch64FloatToUInt(
                        dst_ty,
                        val_operand(src, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::FloatToDouble { src, dst } => {
                instructions.push(AsmInstr::AArch64FloatToDouble(
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::DoubleToFloat { src, dst } => {
                instructions.push(AsmInstr::AArch64DoubleToFloat(
                    val_operand(src, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Unary { op, src, dst } => {
                let ty = asm_type_for_val(dst, types)?;
                let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                if matches!(op, TackyUnaryOp::LogicalNot) {
                    let src_ty = asm_type_for_val(src, types)?;
                    if src_ty == AsmType::Octword {
                        emit_i128_zero_cmp(&mut instructions, src, &stack_slots, global_vars)?;
                        instructions.push(AsmInstr::SetCC(CondCode::E, dst_op));
                        continue;
                    }
                    instructions.push(AsmInstr::Cmp(
                        src_ty,
                        AsmOperand::Imm(0),
                        val_operand(src, &stack_slots, global_vars)?,
                    ));
                    instructions.push(AsmInstr::SetCC(CondCode::E, dst_op));
                    continue;
                }
                if ty == AsmType::LongDouble && matches!(op, TackyUnaryOp::Negate) {
                    emit_long_double_negate(
                        &mut instructions,
                        src,
                        dst,
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    dst_op.clone(),
                ));
                let asm_op = match op {
                    TackyUnaryOp::Negate => AsmUnaryOp::Neg,
                    TackyUnaryOp::Complement => AsmUnaryOp::Not,
                    TackyUnaryOp::LogicalNot => {
                        return Err(
                            "logical-not should be lowered before AArch64 unary emission"
                                .to_string(),
                        )
                    }
                };
                if ty == AsmType::Octword {
                    emit_i128_copy(&mut instructions, src, dst, &stack_slots, global_vars)?;
                    let dst_low = low64_operand(&dst_op)?;
                    let dst_high = high64_operand(&dst_op)?;
                    match asm_op {
                        AsmUnaryOp::Not => {
                            instructions.push(AsmInstr::Unary(
                                AsmType::Quadword,
                                AsmUnaryOp::Not,
                                dst_low,
                            ));
                            instructions.push(AsmInstr::Unary(
                                AsmType::Quadword,
                                AsmUnaryOp::Not,
                                dst_high,
                            ));
                        }
                        AsmUnaryOp::Neg => {
                            instructions.push(AsmInstr::Unary(
                                AsmType::Quadword,
                                AsmUnaryOp::Not,
                                dst_low.clone(),
                            ));
                            instructions.push(AsmInstr::Unary(
                                AsmType::Quadword,
                                AsmUnaryOp::Not,
                                dst_high.clone(),
                            ));
                            instructions.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::AddSetFlags,
                                AsmOperand::Imm(1),
                                dst_low,
                            ));
                            instructions.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                AsmBinaryOp::Adc,
                                AsmOperand::Imm(0),
                                dst_high,
                            ));
                        }
                    }
                    continue;
                }
                instructions.push(AsmInstr::Unary(ty, asm_op, dst_op));
            }
            TackyInstr::Jump(label) => {
                instructions.push(AsmInstr::Jmp(label.clone()));
            }
            TackyInstr::NonlocalJump(label) => {
                if frame_size > 0 {
                    instructions.push(AsmInstr::DeallocateStack(frame_size));
                }
                if large_stack_size > 0 {
                    instructions.push(AsmInstr::AArch64DeallocateLargeStack(large_stack_size));
                }
                instructions.push(AsmInstr::NonlocalJmp(label.clone()));
            }
            TackyInstr::JumpIndirect(target) => {
                instructions.push(AsmInstr::JmpIndirect(val_operand(
                    target,
                    &stack_slots,
                    global_vars,
                )?));
            }
            TackyInstr::JumpIfZero(val, label) => {
                let ty = asm_type_for_val(val, types)?;
                if ty == AsmType::Octword {
                    emit_i128_zero_cmp(&mut instructions, val, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
                    continue;
                }
                instructions.push(AsmInstr::Cmp(
                    ty,
                    AsmOperand::Imm(0),
                    val_operand(val, &stack_slots, global_vars)?,
                ));
                instructions.push(AsmInstr::JmpCC(CondCode::E, label.clone()));
            }
            TackyInstr::JumpIfNotZero(val, label) => {
                let ty = asm_type_for_val(val, types)?;
                if ty == AsmType::Octword {
                    emit_i128_zero_cmp(&mut instructions, val, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
                    continue;
                }
                instructions.push(AsmInstr::Cmp(
                    ty,
                    AsmOperand::Imm(0),
                    val_operand(val, &stack_slots, global_vars)?,
                ));
                instructions.push(AsmInstr::JmpCC(CondCode::NE, label.clone()));
            }
            TackyInstr::Label(label) => {
                instructions.push(AsmInstr::Label(label.clone()));
            }
            TackyInstr::LoadLabelAddress(label, dst) => {
                instructions.push(AsmInstr::LoadLabelAddress(
                    label.clone(),
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::FrameAddress { dst } => {
                instructions.push(AsmInstr::Lea(
                    AsmOperand::Stack(frame_size),
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::BuiltinSetjmp {
                buf,
                dst,
                label,
                end_label,
            } => {
                instructions.push(AsmInstr::BuiltinSetjmp {
                    buf: val_operand(buf, &stack_slots, global_vars)?,
                    dst: val_operand(dst, &stack_slots, global_vars)?,
                    label: label.clone(),
                    end_label: end_label.clone(),
                });
            }
            TackyInstr::BuiltinLongjmp { buf, value } => {
                instructions.push(AsmInstr::BuiltinLongjmp {
                    buf: val_operand(buf, &stack_slots, global_vars)?,
                    value: val_operand(value, &stack_slots, global_vars)?,
                });
            }
            TackyInstr::GetAddress { src, dst } => {
                let TackyVal::Var(name) = src else {
                    return Err(
                        "AArch64 backend can only take addresses of local variables".to_string()
                    );
                };
                if let Some((base_slot, _)) = large_local_offsets.get(name) {
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Stack(*base_slot),
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                } else {
                    instructions.push(AsmInstr::Lea(
                        stack_or_data_operand(name, 0, &stack_slots, global_vars)?,
                        val_operand(dst, &stack_slots, global_vars)?,
                    ));
                }
            }
            TackyInstr::Load { src_ptr, dst } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(src_ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::LoadIndirect(
                    dst_ty,
                    Reg::R11,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Store { src, dst_ptr } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::Mov(
                    AsmType::Quadword,
                    val_operand(dst_ptr, &stack_slots, global_vars)?,
                    AsmOperand::Reg(Reg::R11),
                ));
                instructions.push(AsmInstr::StoreIndirect(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    Reg::R11,
                ));
            }
            TackyInstr::CopyToOffset {
                src,
                dst_name,
                offset,
            } => {
                let src_ty = asm_type_for_val(src, types)?;
                instructions.push(AsmInstr::Mov(
                    src_ty,
                    val_operand(src, &stack_slots, global_vars)?,
                    stack_or_data_operand(dst_name, *offset as i32, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::CopyFromOffset {
                src_name,
                offset,
                dst,
            } => {
                let dst_ty = asm_type_for_val(dst, types)?;
                instructions.push(AsmInstr::Mov(
                    dst_ty,
                    stack_or_data_operand(src_name, *offset as i32, &stack_slots, global_vars)?,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::CopyStruct { src_name, dst_name } => {
                if src_name == dst_name {
                    continue;
                }
                let size = aggregate_size(dst_name, array_sizes, var_struct_tags, struct_defs)
                    .or_else(|| aggregate_size(src_name, array_sizes, var_struct_tags, struct_defs))
                    .ok_or_else(|| {
                        format!(
                            "AArch64 backend missing aggregate size for struct copy {} -> {}",
                            src_name, dst_name
                        )
                    })?;
                copy_bytes(
                    &mut instructions,
                    src_name,
                    dst_name,
                    size,
                    &stack_slots,
                    global_vars,
                )?;
            }
            TackyInstr::VaStart { dst } => {
                instructions.push(AsmInstr::Lea(
                    AsmOperand::Stack(va_start_stack_offset),
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::AddPtr {
                ptr,
                index,
                scale,
                dst,
            } => {
                instructions.push(AsmInstr::AArch64AddPtr(
                    val_operand(ptr, &stack_slots, global_vars)?,
                    val_operand(index, &stack_slots, global_vars)?,
                    *scale,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::FunCall {
                name,
                args,
                dst,
                stack_arg_indices,
                memory_arg_blocks,
                struct_arg_groups,
                variadic,
                fixed_flat_arg_count,
                hidden_return,
                indirect,
            } => {
                let arg_groups: HashMap<usize, (usize, Vec<bool>)> = struct_arg_groups
                    .iter()
                    .map(|(start, count, is_sse)| (*start, (*count, is_sse.clone())))
                    .collect();
                let memory_blocks: HashMap<usize, (usize, usize)> = memory_arg_blocks
                    .iter()
                    .map(|(index, size, align)| (*index, (*size, *align)))
                    .collect();
                enum StackArg<'a> {
                    Scalar(AsmType, &'a TackyVal),
                    MemoryBlock {
                        src_ptr: &'a TackyVal,
                        size: usize,
                        align: usize,
                    },
                }

                impl StackArg<'_> {
                    fn slot_count(&self) -> usize {
                        match self {
                            StackArg::Scalar(AsmType::Octword | AsmType::LongDouble, _) => 2,
                            StackArg::Scalar(_, _) => 1,
                            StackArg::MemoryBlock { size, .. } => {
                                size.div_ceil(STACK_SLOT_SIZE as usize)
                            }
                        }
                    }

                    fn slot_alignment(&self) -> usize {
                        match self {
                            StackArg::Scalar(AsmType::Octword | AsmType::LongDouble, _) => 2,
                            StackArg::MemoryBlock { align, .. } if *align >= 16 => 2,
                            _ => 1,
                        }
                    }
                }

                let mut gp_arg_count = 0usize;
                let mut fp_arg_count = 0usize;
                let mut stack_args = Vec::new();
                let mut arg_index = 0usize;
                while arg_index < args.len() {
                    if let Some((size, align)) = memory_blocks.get(&arg_index).copied() {
                        stack_args.push(StackArg::MemoryBlock {
                            src_ptr: &args[arg_index],
                            size,
                            align,
                        });
                        arg_index += 1;
                        continue;
                    }
                    if let Some((count, is_sse)) = arg_groups.get(&arg_index) {
                        let force_stack_for_darwin_vararg = *variadic
                            && target.os == TargetOs::MacOs
                            && arg_index >= *fixed_flat_arg_count;
                        let (gp_needed, fp_needed) = group_register_needs(is_sse);
                        let fits_registers = gp_arg_count + gp_needed <= ARG_REGS.len()
                            && fp_arg_count + fp_needed <= FP_ARG_REGS.len();
                        for (group_offset, is_fp) in is_sse.iter().copied().enumerate().take(*count)
                        {
                            let arg = &args[arg_index + group_offset];
                            let ty = asm_type_for_val(arg, types)?;
                            if fits_registers && !force_stack_for_darwin_vararg {
                                if is_fp {
                                    instructions.push(AsmInstr::Mov(
                                        ty,
                                        val_operand(arg, &stack_slots, global_vars)?,
                                        AsmOperand::Xmm(FP_ARG_REGS[fp_arg_count]),
                                    ));
                                    fp_arg_count += 1;
                                } else {
                                    instructions.push(AsmInstr::Mov(
                                        ty,
                                        val_operand(arg, &stack_slots, global_vars)?,
                                        AsmOperand::Reg(ARG_REGS[gp_arg_count]),
                                    ));
                                    gp_arg_count += 1;
                                }
                            } else {
                                stack_args.push(StackArg::Scalar(ty, arg));
                            }
                        }
                        arg_index += *count;
                        continue;
                    }

                    let arg = &args[arg_index];
                    let ty = asm_type_for_val(arg, types)?;
                    let force_stack_for_darwin_vararg = *variadic
                        && target.os == TargetOs::MacOs
                        && arg_index >= *fixed_flat_arg_count;
                    if stack_arg_indices.contains(&arg_index) || force_stack_for_darwin_vararg {
                        stack_args.push(StackArg::Scalar(ty, arg));
                    } else if ty == AsmType::Octword && gp_arg_count + 1 < ARG_REGS.len() {
                        let src = val_operand(arg, &stack_slots, global_vars)?;
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            low64_operand(&src)?,
                            AsmOperand::Reg(ARG_REGS[gp_arg_count]),
                        ));
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            high64_operand(&src)?,
                            AsmOperand::Reg(ARG_REGS[gp_arg_count + 1]),
                        ));
                        gp_arg_count += 2;
                    } else if matches!(ty, AsmType::Float | AsmType::Double | AsmType::LongDouble) {
                        if fp_arg_count < FP_ARG_REGS.len() {
                            instructions.push(AsmInstr::Mov(
                                ty,
                                val_operand(arg, &stack_slots, global_vars)?,
                                AsmOperand::Xmm(FP_ARG_REGS[fp_arg_count]),
                            ));
                            fp_arg_count += 1;
                        } else {
                            stack_args.push(StackArg::Scalar(ty, arg));
                        }
                    } else if gp_arg_count < ARG_REGS.len() {
                        instructions.push(AsmInstr::Mov(
                            ty,
                            val_operand(arg, &stack_slots, global_vars)?,
                            AsmOperand::Reg(ARG_REGS[gp_arg_count]),
                        ));
                        gp_arg_count += 1;
                    } else {
                        stack_args.push(StackArg::Scalar(ty, arg));
                    }
                    arg_index += 1;
                }

                let stack_arg_count = stack_args.iter().fold(0usize, |index, arg| {
                    index.next_multiple_of(arg.slot_alignment()) + arg.slot_count()
                });
                let outgoing_bytes = outgoing_stack_size(stack_arg_count);
                if outgoing_bytes > 0 {
                    instructions.push(AsmInstr::AllocateStack(outgoing_bytes));
                    let mut stack_index = 0usize;
                    for arg in &stack_args {
                        stack_index = stack_index.next_multiple_of(arg.slot_alignment());
                        match arg {
                            StackArg::Scalar(AsmType::Octword, val) => {
                                let src = val_operand(val, &stack_slots, global_vars)?;
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    AsmType::Quadword,
                                    low64_operand(&src)?,
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                ));
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    AsmType::Quadword,
                                    high64_operand(&src)?,
                                    stack_arg_offset(0, stack_index + 1),
                                    outgoing_bytes,
                                ));
                                stack_index += arg.slot_count();
                            }
                            StackArg::Scalar(AsmType::LongDouble, val) => {
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    AsmType::LongDouble,
                                    val_operand(val, &stack_slots, global_vars)?,
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                ));
                                stack_index += arg.slot_count();
                            }
                            StackArg::Scalar(ty, val) => {
                                instructions.push(AsmInstr::AArch64StoreOutgoingArg(
                                    *ty,
                                    val_operand(val, &stack_slots, global_vars)?,
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                ));
                                stack_index += arg.slot_count();
                            }
                            StackArg::MemoryBlock { src_ptr, size, .. } => {
                                emit_copy_pointer_to_outgoing_arg(
                                    &mut instructions,
                                    val_operand(src_ptr, &stack_slots, global_vars)?,
                                    *size,
                                    stack_arg_offset(0, stack_index),
                                    outgoing_bytes,
                                );
                                stack_index += arg.slot_count();
                            }
                        }
                    }
                }
                if *indirect {
                    let callee =
                        val_operand(&TackyVal::Var(name.clone()), &stack_slots, global_vars)?;
                    if outgoing_bytes > 0 {
                        instructions.push(AsmInstr::AArch64LoadAdjusted(
                            AsmType::Quadword,
                            callee,
                            Reg::R10,
                            outgoing_bytes,
                        ));
                    } else {
                        instructions.push(AsmInstr::Mov(
                            AsmType::Quadword,
                            callee,
                            AsmOperand::Reg(Reg::R10),
                        ));
                    }
                }
                instructions.push(AsmInstr::Call(
                    name.clone(),
                    args.len(),
                    0,
                    *indirect,
                    false,
                ));
                if outgoing_bytes > 0 {
                    instructions.push(AsmInstr::DeallocateStack(outgoing_bytes));
                }
                if *hidden_return {
                    continue;
                }
                if val_ctype(dst, types) == Some(CType::Struct) {
                    if struct_size_for_val(dst, array_sizes, var_struct_tags, struct_defs)
                        .is_some_and(|size| size > 16)
                    {
                        continue;
                    }
                    let classes = struct_classes_for_val(dst, var_struct_tags, struct_defs)
                        .ok_or_else(|| {
                            "AArch64 backend missing struct class for call return".to_string()
                        })?;
                    move_return_regs_to_struct(
                        &mut instructions,
                        dst,
                        &classes,
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                let dst_ty = asm_type_for_val(dst, types)?;
                if dst_ty == AsmType::Octword {
                    let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::AX),
                        low64_operand(&dst_op)?,
                    ));
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        AsmOperand::Reg(Reg::DI),
                        high64_operand(&dst_op)?,
                    ));
                    continue;
                }
                let ret_src = if matches!(
                    dst_ty,
                    AsmType::Float | AsmType::Double | AsmType::LongDouble
                ) {
                    AsmOperand::Xmm(XmmReg::XMM0)
                } else {
                    AsmOperand::Reg(Reg::AX)
                };
                instructions.push(AsmInstr::Mov(
                    dst_ty,
                    ret_src,
                    val_operand(dst, &stack_slots, global_vars)?,
                ));
            }
            TackyInstr::Binary {
                op,
                left,
                right,
                dst,
            } => {
                let ty = asm_type_for_val(dst, types)?;
                let dst_op = val_operand(dst, &stack_slots, global_vars)?;
                if asm_type_for_val(left, types)? == AsmType::LongDouble
                    || asm_type_for_val(right, types)? == AsmType::LongDouble
                {
                    if let Some(comparison) = long_double_comparison_helper(op) {
                        emit_long_double_comparison(
                            &mut instructions,
                            comparison,
                            left,
                            right,
                            dst,
                            &stack_slots,
                            global_vars,
                        )?;
                        continue;
                    }
                    let Some(helper) = long_double_helper(op) else {
                        return Err(format!(
                            "AArch64 backend does not support long double binary op yet: {:?}",
                            op
                        ));
                    };
                    emit_long_double_helper_call(
                        &mut instructions,
                        helper,
                        left,
                        right,
                        dst,
                        &stack_slots,
                        global_vars,
                    )?;
                    continue;
                }
                if matches!(op, TackyBinaryOp::Equal | TackyBinaryOp::NotEqual)
                    && (asm_type_for_val(left, types)? == AsmType::Octword
                        || asm_type_for_val(right, types)? == AsmType::Octword)
                {
                    let (left_low, left_high) =
                        i128_part_operands(left, &stack_slots, global_vars)?;
                    let (right_low, right_high) =
                        i128_part_operands(right, &stack_slots, global_vars)?;
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        left_low,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        AsmBinaryOp::Xor,
                        right_low,
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Mov(
                        AsmType::Quadword,
                        left_high,
                        AsmOperand::Reg(Reg::R13),
                    ));
                    instructions.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        AsmBinaryOp::Xor,
                        right_high,
                        AsmOperand::Reg(Reg::R13),
                    ));
                    instructions.push(AsmInstr::Binary(
                        AsmType::Quadword,
                        AsmBinaryOp::Or,
                        AsmOperand::Reg(Reg::R13),
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::Cmp(
                        AsmType::Quadword,
                        AsmOperand::Imm(0),
                        AsmOperand::Reg(Reg::R10),
                    ));
                    instructions.push(AsmInstr::SetCC(
                        if matches!(op, TackyBinaryOp::Equal) {
                            CondCode::E
                        } else {
                            CondCode::NE
                        },
                        dst_op,
                    ));
                    continue;
                }
                if matches!(
                    op,
                    TackyBinaryOp::LessThan
                        | TackyBinaryOp::LessEqual
                        | TackyBinaryOp::GreaterThan
                        | TackyBinaryOp::GreaterEqual
                ) && (asm_type_for_val(left, types)? == AsmType::Octword
                    || asm_type_for_val(right, types)? == AsmType::Octword)
                {
                    emit_i128_signed_cmp(&mut instructions, left, right, op, dst_op, &i128_ctx)?;
                    continue;
                }
                if ty == AsmType::Octword {
                    match op {
                        TackyBinaryOp::Add
                        | TackyBinaryOp::Sub
                        | TackyBinaryOp::Mul
                        | TackyBinaryOp::BitwiseAnd
                        | TackyBinaryOp::BitwiseOr
                        | TackyBinaryOp::BitwiseXor
                        | TackyBinaryOp::ShiftLeft
                        | TackyBinaryOp::ShiftRight => {
                            emit_i128_copy(
                                &mut instructions,
                                left,
                                dst,
                                &stack_slots,
                                global_vars,
                            )?;
                            let dst_low = low64_operand(&dst_op)?;
                            let dst_high = high64_operand(&dst_op)?;
                            if matches!(op, TackyBinaryOp::ShiftLeft) {
                                let TackyVal::Constant(amount) = right else {
                                    emit_i128_variable_shift(
                                        &mut instructions,
                                        op,
                                        left,
                                        right,
                                        dst,
                                        &i128_ctx,
                                    )?;
                                    continue;
                                };
                                if *amount == 0 {
                                    continue;
                                }
                                if *amount == 64 {
                                    instructions.push(AsmInstr::Mov(
                                        AsmType::Quadword,
                                        dst_low.clone(),
                                        dst_high,
                                    ));
                                    instructions.push(AsmInstr::Mov(
                                        AsmType::Quadword,
                                        AsmOperand::Imm(0),
                                        dst_low,
                                    ));
                                    continue;
                                }
                                if (1..64).contains(amount) {
                                    instructions.push(AsmInstr::Mov(
                                        AsmType::Quadword,
                                        dst_low.clone(),
                                        AsmOperand::Reg(Reg::R13),
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        AsmBinaryOp::Sal,
                                        AsmOperand::Imm(*amount),
                                        dst_high.clone(),
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        AsmBinaryOp::Shr,
                                        AsmOperand::Imm(64 - *amount),
                                        AsmOperand::Reg(Reg::R13),
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        AsmBinaryOp::Or,
                                        AsmOperand::Reg(Reg::R13),
                                        dst_high,
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        AsmBinaryOp::Sal,
                                        AsmOperand::Imm(*amount),
                                        dst_low,
                                    ));
                                    continue;
                                }
                                emit_i128_variable_shift(
                                    &mut instructions,
                                    op,
                                    left,
                                    right,
                                    dst,
                                    &i128_ctx,
                                )?;
                                continue;
                            }
                            if matches!(op, TackyBinaryOp::ShiftRight) {
                                let TackyVal::Constant(amount) = right else {
                                    emit_i128_variable_shift(
                                        &mut instructions,
                                        op,
                                        left,
                                        right,
                                        dst,
                                        &i128_ctx,
                                    )?;
                                    continue;
                                };
                                let high_shift = if is_unsigned_val(left, types) {
                                    AsmBinaryOp::Shr
                                } else {
                                    AsmBinaryOp::Sar
                                };
                                if *amount == 0 {
                                    continue;
                                }
                                if *amount == 64 {
                                    instructions.push(AsmInstr::Mov(
                                        AsmType::Quadword,
                                        dst_high.clone(),
                                        dst_low,
                                    ));
                                    if is_unsigned_val(left, types) {
                                        instructions.push(AsmInstr::Mov(
                                            AsmType::Quadword,
                                            AsmOperand::Imm(0),
                                            dst_high,
                                        ));
                                    } else {
                                        instructions.push(AsmInstr::Binary(
                                            AsmType::Quadword,
                                            AsmBinaryOp::Sar,
                                            AsmOperand::Imm(63),
                                            dst_high,
                                        ));
                                    }
                                    continue;
                                }
                                if (1..64).contains(amount) {
                                    instructions.push(AsmInstr::Mov(
                                        AsmType::Quadword,
                                        dst_high.clone(),
                                        AsmOperand::Reg(Reg::R13),
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        AsmBinaryOp::Shr,
                                        AsmOperand::Imm(*amount),
                                        dst_low.clone(),
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        AsmBinaryOp::Sal,
                                        AsmOperand::Imm(64 - *amount),
                                        AsmOperand::Reg(Reg::R13),
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        AsmBinaryOp::Or,
                                        AsmOperand::Reg(Reg::R13),
                                        dst_low,
                                    ));
                                    instructions.push(AsmInstr::Binary(
                                        AsmType::Quadword,
                                        high_shift,
                                        AsmOperand::Imm(*amount),
                                        dst_high,
                                    ));
                                    continue;
                                }
                                emit_i128_variable_shift(
                                    &mut instructions,
                                    op,
                                    left,
                                    right,
                                    dst,
                                    &i128_ctx,
                                )?;
                                continue;
                            }
                            let (right_low, right_high) =
                                i128_part_operands(right, &stack_slots, global_vars)?;
                            if matches!(op, TackyBinaryOp::Mul) {
                                let (left_low, left_high) =
                                    i128_part_operands(left, &stack_slots, global_vars)?;
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    left_low,
                                    AsmOperand::Reg(Reg::AX),
                                ));
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    left_high,
                                    AsmOperand::Reg(Reg::DI),
                                ));
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    right_low,
                                    AsmOperand::Reg(Reg::SI),
                                ));
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    right_high,
                                    AsmOperand::Reg(Reg::DX),
                                ));
                                instructions.push(AsmInstr::Call(
                                    "__multi3".to_string(),
                                    4,
                                    0,
                                    false,
                                    false,
                                ));
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    AsmOperand::Reg(Reg::AX),
                                    dst_low,
                                ));
                                instructions.push(AsmInstr::Mov(
                                    AsmType::Quadword,
                                    AsmOperand::Reg(Reg::DI),
                                    dst_high,
                                ));
                                continue;
                            }
                            if matches!(
                                op,
                                TackyBinaryOp::BitwiseAnd
                                    | TackyBinaryOp::BitwiseOr
                                    | TackyBinaryOp::BitwiseXor
                            ) {
                                let asm_op = match op {
                                    TackyBinaryOp::BitwiseAnd => AsmBinaryOp::And,
                                    TackyBinaryOp::BitwiseOr => AsmBinaryOp::Or,
                                    TackyBinaryOp::BitwiseXor => AsmBinaryOp::Xor,
                                    _ => {
                                        return Err("internal error: expected bitwise binary op"
                                            .to_string())
                                    }
                                };
                                instructions.push(AsmInstr::Binary(
                                    AsmType::Quadword,
                                    asm_op.clone(),
                                    right_low,
                                    dst_low,
                                ));
                                instructions.push(AsmInstr::Binary(
                                    AsmType::Quadword,
                                    asm_op,
                                    right_high,
                                    dst_high,
                                ));
                                continue;
                            }
                            instructions.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                if matches!(op, TackyBinaryOp::Add) {
                                    AsmBinaryOp::AddSetFlags
                                } else {
                                    AsmBinaryOp::SubSetFlags
                                },
                                right_low,
                                dst_low,
                            ));
                            instructions.push(AsmInstr::Binary(
                                AsmType::Quadword,
                                if matches!(op, TackyBinaryOp::Add) {
                                    AsmBinaryOp::Adc
                                } else {
                                    AsmBinaryOp::Sbb
                                },
                                right_high,
                                dst_high,
                            ));
                        }
                        TackyBinaryOp::Div | TackyBinaryOp::Mod => {
                            let is_unsigned = is_unsigned_val(left, types);
                            let helper = match (op, is_unsigned) {
                                (TackyBinaryOp::Div, true) => "__udivti3",
                                (TackyBinaryOp::Div, false) => "__divti3",
                                (TackyBinaryOp::Mod, true) => "__umodti3",
                                (TackyBinaryOp::Mod, false) => "__modti3",
                                _ => {
                                    return Err(
                                        "internal error: expected div/mod operation".to_string()
                                    )
                                }
                            };
                            let (left_low, left_high) =
                                i128_part_operands(left, &stack_slots, global_vars)?;
                            let (right_low, right_high) =
                                i128_part_operands(right, &stack_slots, global_vars)?;
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                left_low,
                                AsmOperand::Reg(Reg::AX),
                            ));
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                left_high,
                                AsmOperand::Reg(Reg::DI),
                            ));
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                right_low,
                                AsmOperand::Reg(Reg::SI),
                            ));
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                right_high,
                                AsmOperand::Reg(Reg::DX),
                            ));
                            instructions.push(AsmInstr::Call(
                                helper.to_string(),
                                4,
                                0,
                                false,
                                false,
                            ));
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Reg(Reg::AX),
                                low64_operand(&dst_op)?,
                            ));
                            instructions.push(AsmInstr::Mov(
                                AsmType::Quadword,
                                AsmOperand::Reg(Reg::DI),
                                high64_operand(&dst_op)?,
                            ));
                        }
                        _ => {
                            return Err(format!(
                                "AArch64 backend does not support 128-bit binary op yet: {:?}",
                                op
                            ));
                        }
                    }
                    continue;
                }
                if let Some(cc) = convert_comparison_op(op, is_unsigned_comparison_val(left, types))
                {
                    let left_cmp_ty = asm_type_for_val(left, types)?;
                    let right_cmp_ty = asm_type_for_val(right, types)?;
                    let cmp_ty = match (left_cmp_ty, right_cmp_ty) {
                        (AsmType::Double, _) | (_, AsmType::Double) => AsmType::Double,
                        (AsmType::Float, _) | (_, AsmType::Float) => AsmType::Float,
                        (AsmType::Quadword, _) | (_, AsmType::Quadword) => AsmType::Quadword,
                        (AsmType::Longword, _) | (_, AsmType::Longword) => AsmType::Longword,
                        (AsmType::Word, _) | (_, AsmType::Word) => AsmType::Word,
                        _ => AsmType::Byte,
                    };
                    instructions.push(AsmInstr::Cmp(
                        cmp_ty,
                        val_operand(right, &stack_slots, global_vars)?,
                        val_operand(left, &stack_slots, global_vars)?,
                    ));
                    instructions.push(AsmInstr::SetCC(cc, dst_op));
                    continue;
                }
                let asm_op = match op {
                    TackyBinaryOp::Div if matches!(ty, AsmType::Float | AsmType::Double) => {
                        AsmBinaryOp::DivDouble
                    }
                    TackyBinaryOp::Div => {
                        if is_unsigned_val(dst, types) {
                            AsmBinaryOp::UDiv
                        } else {
                            AsmBinaryOp::SDiv
                        }
                    }
                    TackyBinaryOp::ShiftRight => {
                        if is_unsigned_val(left, types) {
                            AsmBinaryOp::Shr
                        } else {
                            AsmBinaryOp::Sar
                        }
                    }
                    TackyBinaryOp::Mod => {
                        instructions.push(AsmInstr::AArch64Rem(
                            ty,
                            is_unsigned_val(dst, types),
                            val_operand(left, &stack_slots, global_vars)?,
                            val_operand(right, &stack_slots, global_vars)?,
                            dst_op,
                        ));
                        continue;
                    }
                    _ => convert_binary_op(op)?,
                };
                instructions.push(AsmInstr::Mov(
                    ty,
                    val_operand(left, &stack_slots, global_vars)?,
                    dst_op.clone(),
                ));
                instructions.push(AsmInstr::Binary(
                    ty,
                    asm_op,
                    val_operand(right, &stack_slots, global_vars)?,
                    dst_op,
                ));
            }
        }
    }

    rewrite_long_double_immediates(&mut instructions, long_double_consts);

    Ok(AsmFunction {
        name: function.name.clone(),
        global: function.global,
        instructions,
    })
}

pub fn gen(program: &TackyProgram, target: &Target) -> Result<AsmProgram, String> {
    let mut top_level = Vec::new();
    let mut long_double_consts = Vec::new();
    for item in &program.top_level {
        match item {
            TackyTopLevel::Function(function) => {
                let mut function =
                    convert_function(function, target, program, &mut long_double_consts)?;
                rewrite_tls_operands(&mut function, &program.thread_local_vars);
                top_level.push(AsmTopLevel::Function(function));
            }
            TackyTopLevel::StaticVar(sv) => {
                top_level.push(AsmTopLevel::StaticVar(AsmStaticVar {
                    name: sv.name.clone(),
                    global: sv.global,
                    thread_local: sv.thread_local,
                    alignment: sv.alignment,
                    init_values: sv.init_values.clone(),
                }));
            }
            TackyTopLevel::StaticConstant(sc) => {
                top_level.push(AsmTopLevel::StaticConstant(AsmStaticConstant {
                    name: sc.name.clone(),
                    alignment: sc.alignment,
                    init: sc.init.clone(),
                }));
            }
            TackyTopLevel::Alias { name, target } => {
                top_level.push(AsmTopLevel::Alias {
                    name: name.clone(),
                    target: target.clone(),
                });
            }
        }
    }
    for (name, value) in long_double_consts {
        top_level.push(AsmTopLevel::StaticConstant(AsmStaticConstant {
            name,
            alignment: 16,
            init: StaticInit::LongDoubleInit(value),
        }));
    }
    Ok(AsmProgram { top_level })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lex, parse, resolve, tacky};

    fn codegen_source(source: &str) -> Result<AsmProgram, String> {
        let tokens = lex::lex(source)?;
        let ast = parse::parse(tokens)?;
        let resolved = resolve::resolve(ast).map_err(|err| err.render())?.program;
        let tacky = tacky::generate(resolved)?;
        gen(&tacky, &Target::aarch64_linux())
    }

    fn function<'a>(program: &'a AsmProgram, name: &str) -> Result<&'a AsmFunction, String> {
        program
            .top_level
            .iter()
            .find_map(|item| match item {
                AsmTopLevel::Function(function) if function.name == name => Some(function),
                _ => None,
            })
            .ok_or_else(|| format!("function `{name}` should exist"))
    }

    #[test]
    fn grouped_integer_struct_argument_spills_as_a_unit() -> Result<(), String> {
        let program = codegen_source(
            "struct pair { long a; long b; };\n\
             int f(long a, long b, long c, long d, long e, long f, long g, struct pair p) { return p.b; }\n\
             int main(void) { struct pair p = {8, 9}; return f(1, 2, 3, 4, 5, 6, 7, p); }\n",
        )?;
        let main = function(&program, "main")?;
        let outgoing_args = main
            .instructions
            .iter()
            .filter(|instr| matches!(instr, AsmInstr::AArch64StoreOutgoingArg(..)))
            .count();
        assert_eq!(outgoing_args, 2);
        assert!(!main.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(_, _, AsmOperand::Reg(Reg::R12))
                    | AsmInstr::Mov(_, AsmOperand::Reg(Reg::R12), _)
            )
        }));
        Ok(())
    }

    #[test]
    fn function_designator_argument_uses_label_address() -> Result<(), String> {
        let program = codegen_source(
            "int f(int (*fp)(int), int x) { return fp(x); }\n\
             int inc(int x) { return x + 1; }\n\
             int main(void) { return f(inc, 9); }\n",
        )?;
        let main = function(&program, "main")?;
        assert!(main.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Lea(AsmOperand::Data(name), _) if name == "inc"
            )
        }));
        Ok(())
    }

    #[test]
    fn huge_local_array_uses_saved_large_stack_base() -> Result<(), String> {
        let program = codegen_source(
            "extern int sink(char *);\n\
             int f(void) { char s[0x10000000000UL]; return sink(s); }\n",
        )?;
        let function = function(&program, "f")?;
        assert!(function.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::AArch64AllocateLargeStack(bytes) if *bytes == 0x10000000000
            )
        }));
        let base_slot = function.instructions.iter().find_map(|instr| {
            if let AsmInstr::AArch64StoreLargeLocalBase { dst_offset, .. } = instr {
                Some(*dst_offset)
            } else {
                None
            }
        });
        let Some(base_slot) = base_slot else {
            return Err("expected saved large local base".to_string());
        };
        assert!(function.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Mov(AsmType::Quadword, AsmOperand::Stack(slot), _)
                    if *slot == base_slot
            )
        }));
        assert!(!function.instructions.iter().any(|instr| {
            matches!(
                instr,
                AsmInstr::Lea(AsmOperand::Stack(slot), _)
                    if *slot == base_slot
            )
        }));
        Ok(())
    }
}
